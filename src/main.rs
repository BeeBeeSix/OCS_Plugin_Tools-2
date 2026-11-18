#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use acadrust::{CadDocument, DwgReader, DwgWriter, DxfReader, DxfWriter};
use eframe::egui;
use rayon::prelude::*;
use rfd::FileDialog;
use walkdir::WalkDir;

#[derive(Clone)]
struct PurgeOptions {
    regapps: bool,
    blocks: bool,
    xrefs: bool,
    purge_attached_xref_files: bool,
    convert_xrefs_to_relative: bool,
    layers: bool,
    linetypes: bool,
    text_styles: bool,
    dim_styles: bool,
    purge_nested: bool,
}

impl Default for PurgeOptions {
    fn default() -> Self {
        Self {
            regapps: true,
            blocks: false,
            xrefs: false,
            purge_attached_xref_files: false,
            convert_xrefs_to_relative: true,
            layers: false,
            linetypes: false,
            text_styles: false,
            dim_styles: false,
            purge_nested: true,
        }
    }
}

impl PurgeOptions {
    fn any_selected(&self) -> bool {
        self.regapps
            || self.blocks
            || self.xrefs
            || self.layers
            || self.linetypes
            || self.text_styles
            || self.dim_styles
    }
}

#[derive(Clone, Default)]
struct PurgeCounts {
    regapps: usize,
    blocks: usize,
    xrefs: usize,
    layers: usize,
    linetypes: usize,
    text_styles: usize,
    dim_styles: usize,
}

impl PurgeCounts {
    fn total(&self) -> usize {
        self.regapps
            + self.blocks
            + self.xrefs
            + self.layers
            + self.linetypes
            + self.text_styles
            + self.dim_styles
    }
}

#[derive(Clone)]
struct FileReport {
    relative_path: String,
    status: String,
    size_before_bytes: u64,
    size_after_bytes: Option<u64>,
    total_candidates: usize,
    counts: PurgeCounts,
    purged_names: Vec<String>,
    error_message: String,
}

struct SharedState {
    is_running: AtomicBool,
    total_files: AtomicUsize,
    processed_files: AtomicUsize,
    status_text: Mutex<String>,
    last_report_path: Mutex<Option<PathBuf>>,
    reports: Mutex<Vec<FileReport>>,
}

struct PurgeApp {
    input_dir: String,
    output_dir: String,
    dry_run: bool,
    options: PurgeOptions,
    shared: Arc<SharedState>,
}

impl Default for PurgeApp {
    fn default() -> Self {
        Self {
            input_dir: String::new(),
            output_dir: String::new(),
            dry_run: true,
            options: PurgeOptions::default(),
            shared: Arc::new(SharedState {
                is_running: AtomicBool::new(false),
                total_files: AtomicUsize::new(0),
                processed_files: AtomicUsize::new(0),
                status_text: Mutex::new("Select input and output folders to begin.".to_string()),
                last_report_path: Mutex::new(None),
                reports: Mutex::new(Vec::new()),
            }),
        }
    }
}
impl eframe::App for PurgeApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let running = self.shared.is_running.load(Ordering::Relaxed);
        if running {
            ctx.request_repaint();
        }

        // Right-hand sidebar for "Select type"
        egui::SidePanel::right("purge_types_panel")
            .resizable(false)
            .min_width(245.0)
            .show(ctx, |ui| {
                ui.add_space(6.0);
                ui.heading("Select type");
                ui.separator();

                ui.add_enabled_ui(!running, |ui| {
                    ui.horizontal(|ui| {
                        if ui.button("All").clicked() {
                            self.options.regapps = true;
                            self.options.blocks = true;
                            self.options.xrefs = true;
                            self.options.layers = true;
                            self.options.linetypes = true;
                            self.options.text_styles = true;
                            self.options.dim_styles = true;
                        }
                        if ui.button("None").clicked() {
                            self.options.regapps = false;
                            self.options.blocks = false;
                            self.options.xrefs = false;
                            self.options.layers = false;
                            self.options.linetypes = false;
                            self.options.text_styles = false;
                            self.options.dim_styles = false;
                        }
                    });

                    ui.add_space(6.0);
                    ui.checkbox(&mut self.options.regapps, "Registered Apps (RegApps)");
                    ui.separator();
                    ui.label(egui::RichText::new("Named Items Not Used").strong());
                    ui.checkbox(&mut self.options.blocks, "Blocks");
                    ui.checkbox(&mut self.options.xrefs, "Xrefs (unreferenced)");
                    ui.checkbox(&mut self.options.layers, "Layers");
                    ui.checkbox(&mut self.options.linetypes, "Linetypes");
                    ui.checkbox(&mut self.options.text_styles, "Text styles");
                    ui.checkbox(&mut self.options.dim_styles, "Dimension styles");

                    ui.add_space(10.0);
                    ui.separator();
                    ui.label(egui::RichText::new("Options").strong());
                    ui.checkbox(&mut self.options.purge_nested, "Purge nested items");
                    ui.checkbox(
                        &mut self.options.purge_attached_xref_files,
                        "Also open & purge attached Xref files",
                    );
                    ui.checkbox(
                        &mut self.options.convert_xrefs_to_relative,
                        "Convert Xref paths to relative",
                    );
                });
            });

        // Main left panel
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("DWG / DXF Batch RegApp & Item Purger");
            ui.add_space(8.0);

            let box_width = (ui.available_width() - 180.0).max(180.0);

            egui::Grid::new("folder_grid")
                .num_columns(3)
                .spacing([8.0, 8.0])
                .show(ui, |ui| {
                    if ui.add_enabled(!running, egui::Button::new("Browse...")).clicked() {
                        if let Some(folder) = FileDialog::new().pick_folder() {
                            self.input_dir = folder.display().to_string();
                            if self.output_dir.trim().is_empty() {
                                self.output_dir = folder.join("CLEAN").display().to_string();
                            }
                        }
                    }
                    ui.label("Input Folder:");
                    ui.add_enabled(
                        !running,
                        egui::TextEdit::singleline(&mut self.input_dir).desired_width(box_width),
                    );
                    ui.end_row();

                    if ui.add_enabled(!running, egui::Button::new("Browse...")).clicked() {
                        if let Some(folder) = FileDialog::new().pick_folder() {
                            self.output_dir = folder.display().to_string();
                        }
                    }
                    ui.label("Output Folder:");
                    ui.add_enabled(
                        !running,
                        egui::TextEdit::singleline(&mut self.output_dir).desired_width(box_width),
                    );
                    ui.end_row();
                });

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.add_enabled(
                    !running,
                    egui::Checkbox::new(&mut self.dry_run, "Dry Run (preview & CSV report only)"),
                );

                ui.add_space(12.0);

                let can_start = !running
                    && self.options.any_selected()
                    && !self.input_dir.trim().is_empty()
                    && !self.output_dir.trim().is_empty();

                if ui
                    .add_enabled(can_start, egui::Button::new("Start Batch Purge"))
                    .clicked()
                {
                    self.start_batch();
                }

                let out_path = PathBuf::from(self.output_dir.trim());
                let can_open_out =
                    !running && !self.output_dir.trim().is_empty() && out_path.exists();
                if ui
                    .add_enabled(can_open_out, egui::Button::new("Open Output Folder"))
                    .clicked()
                {
                    let _ = Command::new("explorer").arg(&out_path).spawn();
                }

                let report_opt = self.shared.last_report_path.lock().unwrap().clone();
                let can_open_csv = !running && report_opt.as_ref().is_some_and(|p| p.exists());
                if ui
                    .add_enabled(can_open_csv, egui::Button::new("Open CSV Report"))
                    .clicked()
                {
                    if let Some(csv_path) = report_opt {
                        let _ = Command::new("explorer").arg(csv_path).spawn();
                    }
                }
            });

            ui.add_space(10.0);

            let total = self.shared.total_files.load(Ordering::Relaxed);
            let done = self.shared.processed_files.load(Ordering::Relaxed);
            let progress = if total > 0 {
                done as f32 / total as f32
            } else {
                0.0
            };

            ui.add(
                egui::ProgressBar::new(progress)
                    .text(format!("{} / {} files", done, total))
                    .animate(running),
            );

            let status = self.shared.status_text.lock().unwrap().clone();
            ui.label(status);
            ui.separator();

            let rows = self.shared.reports.lock().unwrap().clone();
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    egui::Grid::new("results_grid")
                        .striped(true)
                        .num_columns(7)
                        .spacing([10.0, 4.0])
                        .show(ui, |ui| {
                            ui.strong("File");
                            ui.strong("Status");
                            ui.strong("Before");
                            ui.strong("After");
                            ui.strong("Total");
                            ui.strong("Purged");
                            ui.strong("Details");
                            ui.end_row();

                            for row in &rows {
                                ui.label(&row.relative_path);
                                ui.label(&row.status);
                                ui.label(format_kb(row.size_before_bytes));
                                match row.size_after_bytes {
                                    Some(after) => ui.label(format_kb(after)),
                                    None => ui.label("-"),
                                };
                                ui.label(row.total_candidates.to_string());
                                ui.label(row.counts.total().to_string());
                                if !row.error_message.is_empty() {
                                    ui.colored_label(egui::Color32::RED, &row.error_message);
                                } else {
                                    ui.label(row.purged_names.join(", "));
                                }
                                ui.end_row();
                            }
                        });
                });
        });
    }
}

impl PurgeApp {
    fn start_batch(&self) {
        let input_dir = PathBuf::from(self.input_dir.trim());
        let output_dir = PathBuf::from(self.output_dir.trim());
        let dry_run = self.dry_run;
        let options = self.options.clone();
        let shared = Arc::clone(&self.shared);

        shared.is_running.store(true, Ordering::Relaxed);
        shared.processed_files.store(0, Ordering::Relaxed);
        shared.total_files.store(0, Ordering::Relaxed);
        *shared.last_report_path.lock().unwrap() = None;
        shared.reports.lock().unwrap().clear();
        *shared.status_text.lock().unwrap() = "Scanning folders...".to_string();

        thread::spawn(move || {
            let result = run_batch(&input_dir, &output_dir, dry_run, &options, &shared);
            if let Err(err) = result {
                *shared.status_text.lock().unwrap() = format!("Batch failed: {}", err);
            }
            shared.is_running.store(false, Ordering::Relaxed);
		});
	}
}
fn format_kb(bytes: u64) -> String {
    format!("{:.1} KB", bytes as f64 / 1024.0)
}

fn normalize_path(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.components().collect())
}

fn is_inside_dir(candidate: &Path, root_norm: &Path) -> bool {
    normalize_path(candidate).starts_with(root_norm)
}

fn resolve_xref_path(host_file: &Path, raw_xref_path: &str) -> Option<PathBuf> {
    let trimmed = raw_xref_path.trim();
    if trimmed.is_empty() {
        return None;
    }

    let normalized_str = trimmed.replace('\\', "/");
    let candidate = Path::new(&normalized_str);

    let resolved = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else if let Some(parent) = host_file.parent() {
        parent.join(candidate)
    } else {
        candidate.to_path_buf()
    };

    if resolved.is_file() {
        return Some(resolved);
    }

    if let (Some(parent), Some(file_name)) = (host_file.parent(), candidate.file_name()) {
        let same_dir = parent.join(file_name);
        if same_dir.is_file() {
            return Some(same_dir);
        }
    }

    None
}

fn compute_relative_xref_path(from_dir: &Path, to_file: &Path) -> String {
    let from_norm = normalize_path(from_dir);
    let to_parent_norm = to_file
        .parent()
        .map(normalize_path)
        .unwrap_or_else(|| normalize_path(to_file));
    let file_name = to_file.file_name().unwrap_or_default();

    let from_comps: Vec<_> = from_norm.components().collect();
    let to_comps: Vec<_> = to_parent_norm.components().collect();

    if from_comps.first() != to_comps.first() {
        return to_file.to_string_lossy().replace('/', "\\");
    }

    let mut common = 0;
    while common < from_comps.len()
        && common < to_comps.len()
        && from_comps[common] == to_comps[common]
    {
        common += 1;
    }

    let mut rel = PathBuf::new();
    for _ in common..from_comps.len() {
        rel.push("..");
    }
    for comp in &to_comps[common..] {
        rel.push(comp.as_os_str());
    }
    rel.push(file_name);

    let mut formatted = rel.to_string_lossy().replace('/', "\\");
    if !formatted.starts_with("..\\") && !formatted.starts_with(".\\") {
        formatted = format!(".\\{}", formatted);
    }
    formatted
}

fn target_output_path(
    source_path: &Path,
    input_root: &Path,
    input_norm: &Path,
    output_root: &Path,
) -> PathBuf {
    if is_inside_dir(source_path, input_norm) {
        let rel_path = source_path.strip_prefix(input_root).unwrap_or(source_path);
        output_root.join(rel_path)
    } else {
        let file_name = source_path.file_name().unwrap_or_default();
        output_root.join("_External_Xrefs").join(file_name)
    }
}

fn run_batch(
    input_dir: &Path,
    output_dir: &Path,
    dry_run: bool,
    options: &PurgeOptions,
    shared: &Arc<SharedState>,
) -> Result<(), Box<dyn std::error::Error>> {
    let report_path = output_dir.join("purge_report.csv");

    if !dry_run {
        fs::create_dir_all(output_dir)?;
    }
    if let Some(parent) = report_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let input_norm = normalize_path(input_dir);
    let output_norm = normalize_path(output_dir);

    let initial_files: Vec<PathBuf> = WalkDir::new(input_dir)
        .into_iter()
        .filter_entry(|entry| !is_inside_dir(entry.path(), &output_norm))
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| entry.into_path())
        .filter(|path| {
            matches!(
                path.extension()
                    .and_then(|s| s.to_str())
                    .map(|s| s.to_ascii_lowercase())
                    .as_deref(),
                Some("dwg" | "dxf")
            )
        })
        .collect();

    let visited: Mutex<HashSet<PathBuf>> = Mutex::new(
        initial_files.iter().map(|p| normalize_path(p)).collect(),
    );
    let mut wave: Vec<(PathBuf, bool)> = initial_files.into_iter().map(|p| (p, false)).collect();
    shared.total_files.store(wave.len(), Ordering::Relaxed);

    let pool = rayon::ThreadPoolBuilder::new().num_threads(4).build()?;

    while !wave.is_empty() {
        *shared.status_text.lock().unwrap() = format!("Processing {} file(s)...", wave.len());
        let next_wave: Mutex<Vec<(PathBuf, bool)>> = Mutex::new(Vec::new());

        pool.install(|| {
            wave.par_iter().for_each(|(path, is_external_xref)| {
                let display_path = if *is_external_xref {
                    format!("[XREF] {}", path.display())
                } else {
                    path.strip_prefix(input_dir)
                        .unwrap_or(path)
                        .to_string_lossy()
                        .to_string()
                };

                let size_before = fs::metadata(path).map(|m| m.len()).unwrap_or(0);

                let report = match process_file(
                    path,
                    input_dir,
                    &input_norm,
                    output_dir,
                    dry_run,
                    options,
                ) {
                    Ok(outcome) => {
                        if options.purge_attached_xref_files {
                            let mut seen = visited.lock().unwrap();
                            let mut queued = next_wave.lock().unwrap();
                            for xref_path in outcome.attached_xref_paths {
                                let norm = normalize_path(&xref_path);
                                if seen.insert(norm) {
                                    let external = !is_inside_dir(&xref_path, &input_norm);
                                    queued.push((xref_path, external));
                                    shared.total_files.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }

                        FileReport {
                            relative_path: display_path,
                            status: if dry_run { "DRY_RUN" } else { "CLEANED" }.to_string(),
                            size_before_bytes: size_before,
                            size_after_bytes: outcome.size_after_bytes,
                            total_candidates: outcome.total_candidates,
                            counts: outcome.counts,
                            purged_names: outcome.purged_names,
                            error_message: String::new(),
                        }
                    }
                    Err(err) => FileReport {
                        relative_path: display_path,
                        status: "ERROR".to_string(),
                        size_before_bytes: size_before,
                        size_after_bytes: None,
                        total_candidates: 0,
                        counts: PurgeCounts::default(),
                        purged_names: Vec::new(),
                        error_message: err.to_string(),
                    },
                };

                shared.reports.lock().unwrap().push(report);
                shared.processed_files.fetch_add(1, Ordering::Relaxed);
            });
        });

        wave = next_wave.into_inner().unwrap();
    }

    let mut final_reports = shared.reports.lock().unwrap().clone();
    final_reports.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    *shared.reports.lock().unwrap() = final_reports.clone();

    write_csv_report(&report_path, &final_reports)?;
    *shared.last_report_path.lock().unwrap() = Some(report_path.clone());

    *shared.status_text.lock().unwrap() = format!(
        "Done! Processed {} file(s). Report saved to {}",
        final_reports.len(),
        report_path.display()
    );

    Ok(())
}
struct FileOutcome {
    size_after_bytes: Option<u64>,
    total_candidates: usize,
    counts: PurgeCounts,
    purged_names: Vec<String>,
    attached_xref_paths: Vec<PathBuf>,
}

fn process_file(
    input_path: &Path,
    input_root: &Path,
    input_norm: &Path,
    output_root: &Path,
    dry_run: bool,
    options: &PurgeOptions,
) -> acadrust::Result<FileOutcome> {
    let ext = input_path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    let mut doc = match ext.as_str() {
        "dwg" => DwgReader::from_file(input_path)?.read()?,
        "dxf" => DxfReader::from_file(input_path)?.read()?,
        _ => {
            return Ok(FileOutcome {
                size_after_bytes: None,
                total_candidates: 0,
                counts: PurgeCounts::default(),
                purged_names: Vec::new(),
                attached_xref_paths: Vec::new(),
            });
        }
    };

    let out_path = target_output_path(input_path, input_root, input_norm, output_root);

    // Collect attached Xref file paths and optionally convert paths to relative
    let mut attached_xref_paths: Vec<PathBuf> = Vec::new();
    let host_out_dir = out_path.parent().map(|p| p.to_path_buf());

    for b in doc.block_records.iter_mut() {
        if b.flags.is_xref {
            if let Some(resolved_src) = resolve_xref_path(input_path, &b.xref_path) {
                if options.purge_attached_xref_files {
                    attached_xref_paths.push(resolved_src.clone());
                }

                if options.convert_xrefs_to_relative {
                    if let Some(ref out_dir) = host_out_dir {
                        let target_xref = if options.purge_attached_xref_files {
                            target_output_path(&resolved_src, input_root, input_norm, output_root)
                        } else {
                            resolved_src
                        };
                        b.xref_path = compute_relative_xref_path(out_dir, &target_xref);
                    }
                }
            }
        }
    }

    let mut total_candidates = 0;
    if options.regapps {
        total_candidates += doc.app_ids.len();
    }
    if options.blocks {
        total_candidates += doc.block_records.iter().filter(|b| !b.flags.is_xref).count();
    }
    if options.xrefs {
        total_candidates += doc.block_records.iter().filter(|b| b.flags.is_xref).count();
    }
    if options.layers {
        total_candidates += doc.layers.len();
    }
    if options.linetypes {
        total_candidates += doc.line_types.len();
    }
    if options.text_styles {
        total_candidates += doc.text_styles.len();
    }
    if options.dim_styles {
        total_candidates += doc.dim_styles.len();
    }

    let mut counts = PurgeCounts::default();
    let mut purged_log: Vec<String> = Vec::new();
    let max_passes = if options.purge_nested { 10 } else { 1 };

    for _pass in 0..max_passes {
        let mut removed_this_pass = 0;

        // 1. Purge unreferenced Blocks and/or Xrefs
        if options.blocks || options.xrefs {
            let mut used_blocks: HashSet<String> = HashSet::new();
            used_blocks.insert("*MODEL_SPACE".to_string());
            used_blocks.insert("*PAPER_SPACE".to_string());

            for entity in doc.entities() {
                if let acadrust::EntityType::Insert(ins) = entity {
                    used_blocks.insert(ins.block_name.to_ascii_uppercase());
                }
            }

            let unreferenced: Vec<(String, bool)> = doc
                .block_records
                .iter()
                .map(|b| (b.name.clone(), b.flags.is_xref))
                .filter(|(name, is_xref)| {
                    let upper = name.to_ascii_uppercase();
                    let type_enabled = if *is_xref { options.xrefs } else { options.blocks };
                    type_enabled
                        && !upper.starts_with("*MODEL_SPACE")
                        && !upper.starts_with("*PAPER_SPACE")
                        && !used_blocks.contains(&upper)
                })
                .collect();

            for (name, is_xref) in unreferenced {
                doc.block_records.remove(&name);
                if is_xref {
                    purged_log.push(format!("Xref:{}", name));
                    counts.xrefs += 1;
                } else {
                    purged_log.push(format!("Block:{}", name));
                    counts.blocks += 1;
                }
                removed_this_pass += 1;
            }
        }

        // 2. Purge unreferenced Layers
        if options.layers {
            let mut used_layers: HashSet<String> = HashSet::new();
            used_layers.insert("0".to_string());
            used_layers.insert("DEFPOINTS".to_string());

            for entity in doc.entities() {
                used_layers.insert(entity.common().layer.to_ascii_uppercase());
            }

            let unreferenced: Vec<String> = doc
                .layers
                .iter()
                .map(|l| l.name.clone())
                .filter(|name| !used_layers.contains(&name.to_ascii_uppercase()))
                .collect();

            for name in unreferenced {
                doc.layers.remove(&name);
                purged_log.push(format!("Layer:{}", name));
                counts.layers += 1;
                removed_this_pass += 1;
            }
        }

        // 3. Purge unreferenced Linetypes
        if options.linetypes {
            let mut used_linetypes: HashSet<String> = HashSet::new();
            used_linetypes.insert("BYBLOCK".to_string());
            used_linetypes.insert("BYLAYER".to_string());
            used_linetypes.insert("CONTINUOUS".to_string());

            for entity in doc.entities() {
                used_linetypes.insert(entity.common().linetype.to_ascii_uppercase());
            }
            for layer in doc.layers.iter() {
                used_linetypes.insert(layer.line_type.to_ascii_uppercase());
            }

            let unreferenced: Vec<String> = doc
                .line_types
                .iter()
                .map(|lt| lt.name.clone())
                .filter(|name| !used_linetypes.contains(&name.to_ascii_uppercase()))
                .collect();

            for name in unreferenced {
                doc.line_types.remove(&name);
                purged_log.push(format!("Linetype:{}", name));
                counts.linetypes += 1;
                removed_this_pass += 1;
            }
        }

        // 4. Purge unreferenced Text Styles
        if options.text_styles {
            let mut used_styles: HashSet<String> = HashSet::new();
            used_styles.insert("STANDARD".to_string());

            for entity in doc.entities() {
                match entity {
                    acadrust::EntityType::Text(t) => {
                        used_styles.insert(t.style.to_ascii_uppercase());
                    }
                    acadrust::EntityType::MText(mt) => {
                        used_styles.insert(mt.style.to_ascii_uppercase());
                    }
                    _ => {}
                }
            }

            let unreferenced: Vec<String> = doc
                .text_styles
                .iter()
                .map(|ts| ts.name.clone())
                .filter(|name| !name.is_empty() && !used_styles.contains(&name.to_ascii_uppercase()))
                .collect();

            for name in unreferenced {
                doc.text_styles.remove(&name);
                purged_log.push(format!("TextStyle:{}", name));
                counts.text_styles += 1;
                removed_this_pass += 1;
            }
        }

        // 5. Purge unreferenced Dimension Styles
        if options.dim_styles {
            let mut used_dims: HashSet<String> = HashSet::new();
            used_dims.insert("STANDARD".to_string());

            for entity in doc.entities() {
                if let acadrust::EntityType::Dimension(d) = entity {
                    used_dims.insert(d.base().style_name.to_ascii_uppercase());
                }
            }

            let unreferenced: Vec<String> = doc
                .dim_styles
                .iter()
                .map(|ds| ds.name.clone())
                .filter(|name| !used_dims.contains(&name.to_ascii_uppercase()))
                .collect();

            for name in unreferenced {
                doc.dim_styles.remove(&name);
                purged_log.push(format!("DimStyle:{}", name));
                counts.dim_styles += 1;
                removed_this_pass += 1;
            }
        }

        // 6. Purge unreferenced RegApps
        if options.regapps {
            let (referenced_names, referenced_handles) = collect_referenced_app_ids(&doc);
            let unreferenced: Vec<String> = doc
                .app_ids
                .iter()
                .map(|app| (app.name.clone(), app.handle.value()))
                .filter(|(name, handle)| {
                    !referenced_names.contains(&name.to_ascii_uppercase())
                        && !referenced_handles.contains(handle)
                })
                .map(|(name, _)| name)
                .collect();

            for name in unreferenced {
                doc.app_ids.remove(&name);
                purged_log.push(format!("RegApp:{}", name));
                counts.regapps += 1;
                removed_this_pass += 1;
            }
        }

        if removed_this_pass == 0 {
            break;
        }
    }

    purged_log.sort();

    if dry_run {
        return Ok(FileOutcome {
            size_after_bytes: None,
            total_candidates,
            counts,
            purged_names: purged_log,
            attached_xref_paths,
        });
    }

    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent)?;
    }

    match ext.as_str() {
        "dwg" => DwgWriter::write_to_file(&out_path, &doc)?,
        "dxf" => DxfWriter::new(&doc).write_to_file(&out_path)?,
        _ => {}
    }

    let size_after = fs::metadata(&out_path).map(|m| m.len()).ok();

    Ok(FileOutcome {
        size_after_bytes: size_after,
        total_candidates,
        counts,
        purged_names: purged_log,
        attached_xref_paths,
    })
}

fn collect_referenced_app_ids(doc: &CadDocument) -> (HashSet<String>, HashSet<u64>) {
    let mut used_names = HashSet::new();
    let mut used_handles = HashSet::new();

    used_names.insert("ACAD".to_string());
    used_names.insert("ACAD_MLEADERVER".to_string());

    for entity in doc.entities() {
        let ed = &entity.common().extended_data;

        for record in ed.records() {
            used_names.insert(record.application_name.to_ascii_uppercase());
        }

        for (handle, _) in &ed.raw_dwg_eed {
            used_handles.insert(*handle);
        }
    }

    (used_names, used_handles)
}

fn write_csv_report(report_path: &Path, rows: &[FileReport]) -> std::io::Result<()> {
    let file = File::create(report_path)?;
    let mut writer = BufWriter::new(file);

    writeln!(
        writer,
        "File,Status,Size_Before_Bytes,Size_After_Bytes,Saved_Bytes,Size_Before_KB,Size_After_KB,Total_Candidates,Total_Purged,RegApps_Purged,Blocks_Purged,Xrefs_Purged,Layers_Purged,Linetypes_Purged,TextStyles_Purged,DimStyles_Purged,Purged_Item_Names,Error"
    )?;

    for row in rows {
        let after_bytes_str = row.size_after_bytes.map(|b| b.to_string()).unwrap_or_default();
        let saved_bytes_str = row
            .size_after_bytes
            .map(|after| row.size_before_bytes.saturating_sub(after).to_string())
            .unwrap_or_default();
        let before_kb = format!("{:.2}", row.size_before_bytes as f64 / 1024.0);
        let after_kb = row
            .size_after_bytes
            .map(|b| format!("{:.2}", b as f64 / 1024.0))
            .unwrap_or_default();
        let joined_names = row.purged_names.join("; ");

        writeln!(
            writer,
            "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
            csv_escape(&row.relative_path),
            csv_escape(&row.status),
            row.size_before_bytes,
            after_bytes_str,
            saved_bytes_str,
            before_kb,
            after_kb,
            row.total_candidates,
            row.counts.total(),
            row.counts.regapps,
            row.counts.blocks,
            row.counts.xrefs,
            row.counts.layers,
            row.counts.linetypes,
            row.counts.text_styles,
            row.counts.dim_styles,
            csv_escape(&joined_names),
            csv_escape(&row.error_message),
        )?;
    }

    writer.flush()
}

fn csv_escape(field: &str) -> String {
    if field.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1020.0, 580.0])
            .with_min_inner_size([780.0, 440.0]),
        ..Default::default()
    };

    eframe::run_native(
        "DWG / DXF RegApp & Item Purger",
        options,
        Box::new(|_cc| Ok(Box::<PurgeApp>::default())),
    )
}
