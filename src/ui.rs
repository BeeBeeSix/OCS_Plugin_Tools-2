//! The Steal window. It runs on its own thread in the plugin process, next to the CAD
//! window (Open CAD Studio has no usable plugin dialog API), and edits the drawing through
//! the host's request channel. winit allows one event loop per process, so the window is
//! created once and afterwards only hidden and shown.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};

use eframe::egui::{self, Color32, RichText, Vec2};
use ocs_plugin_api::host::acadrust::Document;

use crate::model::{catalog_of, import_selected, read_source, Bulk, Cat, Catalog, Existing, Report, Selection};
use crate::{log, SenderOps};

pub enum UiMsg {
    Show,
}

struct Handle {
    tx: Sender<UiMsg>,
}

fn handle() -> &'static Mutex<Option<Handle>> {
    static H: OnceLock<Mutex<Option<Handle>>> = OnceLock::new();
    H.get_or_init(|| Mutex::new(None))
}

static CTX: OnceLock<egui::Context> = OnceLock::new();

fn send(msg: UiMsg) -> bool {
    let guard = handle().lock().unwrap_or_else(|e| e.into_inner());
    let Some(h) = guard.as_ref() else { return false };
    let ok = h.tx.send(msg).is_ok();
    if let Some(ctx) = CTX.get() {
        ctx.request_repaint();
    }
    ok
}

/// Show the Steal window, creating it on first use.
pub fn open() -> Result<(), String> {
    if send(UiMsg::Show) {
        return Ok(());
    }
    if cfg!(target_os = "macos") {
        return Err("the Steal window is not available on macOS yet".into());
    }
    let (tx, rx) = channel();
    tx.send(UiMsg::Show).ok();
    *handle().lock().unwrap_or_else(|e| e.into_inner()) = Some(Handle { tx });
    std::thread::Builder::new()
        .name("steal-ui".into())
        .spawn(move || {
            let mut options = eframe::NativeOptions {
                viewport: egui::ViewportBuilder::default()
                    .with_title("Steal - import from another drawing")
                    .with_app_id("ocs-steal")
                    .with_inner_size([940.0, 680.0])
                    .with_min_inner_size([680.0, 440.0]),
                ..Default::default()
            };
            options.event_loop_builder = Some(Box::new(|builder| {
                // The plugin's main thread serves the host IPC; the window lives on this one.
                #[cfg(all(unix, not(target_os = "macos")))]
                winit::platform::x11::EventLoopBuilderExtX11::with_any_thread(builder, true);
                #[cfg(windows)]
                winit::platform::windows::EventLoopBuilderExtWindows::with_any_thread(builder, true);
                let _ = builder;
            }));
            let result = eframe::run_native(
                "Steal",
                options,
                Box::new(move |cc| {
                    let _ = CTX.set(cc.egui_ctx.clone());
                    Ok(Box::new(Shell { app: App::new(Some(rx)) }))
                }),
            );
            *handle().lock().unwrap_or_else(|e| e.into_inner()) = None;
            if let Err(e) = result {
                log(&format!("the Steal window could not start: {e}"));
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

struct Shell {
    app: App,
}

impl eframe::App for Shell {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.app.poll(ctx);
        // Closing only hides: the event loop cannot be created twice.
        if ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.app.ui(ui);
    }
}

// ----------------------------------------------------------------------
// State
// ----------------------------------------------------------------------
/// The source drawing as read, with what it contains.
pub struct Loaded {
    pub file: String,
    pub doc: Document,
    pub catalog: Catalog,
}

enum Job {
    Read(Result<(Loaded, Option<Existing>, Option<String>), String>),
    Imported { report: Report, existing: Option<Existing> },
}

pub struct App {
    rx: Option<Receiver<UiMsg>>,
    jobs_tx: Sender<Job>,
    jobs_rx: Receiver<Job>,
    pub source_path: String,
    pub status: String,
    pub status_is_error: bool,
    pub loaded: Option<Arc<Loaded>>,
    pub existing: Existing,
    pub cat: Cat,
    pub sel: Selection,
    pub hide_existing: bool,
    pub needs: bool,
    pub overwrite: bool,
    pub report: Vec<String>,
    pub busy: bool,
}

impl App {
    pub fn new(rx: Option<Receiver<UiMsg>>) -> Self {
        let (jobs_tx, jobs_rx) = channel();
        App {
            rx,
            jobs_tx,
            jobs_rx,
            source_path: String::new(),
            status: "Choose a source drawing (DXF or DWG) and press Read.".into(),
            status_is_error: false,
            loaded: None,
            existing: Existing::default(),
            cat: Cat::Layers,
            sel: Selection::default(),
            hide_existing: false,
            needs: true,
            overwrite: false,
            report: Vec::new(),
            busy: false,
        }
    }

    fn say(&mut self, text: impl Into<String>, error: bool) {
        self.status = text.into();
        self.status_is_error = error;
    }

    /// Take a source into use (also the tail of the background read).
    pub fn set_loaded(&mut self, loaded: Loaded, existing: Option<Existing>) {
        let summary = format!(
            "{}: {} layers, {} linetypes, {} text styles, {} dimension styles, {} blocks",
            loaded.file,
            loaded.catalog.layers.len(),
            loaded.catalog.linetypes.len(),
            loaded.catalog.text_styles.len(),
            loaded.catalog.dim_styles.len(),
            loaded.catalog.blocks.len()
        );
        self.sel = Selection::default();
        if let Some(e) = existing {
            self.existing = e;
        }
        self.loaded = Some(Arc::new(loaded));
        self.say(summary, false);
    }

    /// Messages from the plugin thread and finished background jobs.
    pub fn poll(&mut self, ctx: &egui::Context) {
        let mut show = false;
        if let Some(rx) = &self.rx {
            while let Ok(msg) = rx.try_recv() {
                match msg {
                    UiMsg::Show => show = true,
                }
            }
        }
        if show {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        while let Ok(job) = self.jobs_rx.try_recv() {
            self.busy = false;
            match job {
                Job::Read(Ok((loaded, existing, warning))) => {
                    self.set_loaded(loaded, existing);
                    if let Some(w) = warning {
                        self.status = format!("{}  -  {w}", self.status);
                    }
                }
                Job::Read(Err(e)) => {
                    log(&format!("read failed: {e}"));
                    self.say(format!("Cannot read the drawing: {e}"), true);
                }
                Job::Imported { report, existing } => {
                    log(&format!("import finished: {} ok, {} failed", report.ok, report.failed));
                    if let Some(e) = existing {
                        self.existing = e;
                    }
                    self.sel.clear();
                    let mut lines = report.lines.clone();
                    lines.extend(report.notes.iter().cloned());
                    lines.push(report.summary());
                    self.report = lines;
                    let failed = report.failed > 0;
                    self.say(report.summary(), failed);
                }
            }
        }
    }

    /// Read the source drawing and the open drawing's names in the background.
    pub fn start_read(&mut self, ctx: &egui::Context) {
        let path = self.source_path.trim().trim_matches('"').to_string();
        if path.is_empty() {
            self.say("Type or browse to a source drawing first.", true);
            return;
        }
        self.busy = true;
        self.say(format!("Reading {path} ..."), false);
        log(&format!("reading {path}"));
        let (tx, ctx) = (self.jobs_tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let result = read_source(&path).map(|doc| {
                let catalog = catalog_of(&doc);
                let file = std::path::Path::new(&path)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.clone());
                let (existing, warning) = match crate::target() {
                    Some(t) => match crate::existing_names(&t.sender) {
                        Ok(e) => (Some(e), None),
                        Err(e) => (None, Some(format!("could not read the open drawing's names ({e})"))),
                    },
                    None => (None, Some("no drawing attached - run STEAL from a drawing tab".to_string())),
                };
                (Loaded { file, doc, catalog }, existing, warning)
            });
            let _ = tx.send(Job::Read(result));
            ctx.request_repaint();
        });
    }

    /// Import everything ticked, in the background, as one undo step.
    pub fn start_import(&mut self, ctx: &egui::Context) {
        let Some(loaded) = self.loaded.clone() else {
            self.say("Read a source drawing first.", true);
            return;
        };
        if self.sel.total() == 0 {
            self.say("Nothing is ticked.", true);
            return;
        }
        let Some(target) = crate::target() else {
            self.say("No drawing attached - run STEAL from a drawing tab, then try again.", true);
            return;
        };
        let choice = self.sel.choice(&loaded.catalog, self.overwrite, self.needs);
        let mut existing = self.existing.clone();
        self.busy = true;
        self.say("Importing ...", false);
        log("import started");
        let (tx, ctx) = (self.jobs_tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let mut ops = SenderOps { sender: target.sender.clone() };
            let report = import_selected(&mut ops, &loaded.doc, &mut existing, &choice);
            // the host's own list is authoritative; fall back to what we tracked
            let fresh = crate::existing_names(&target.sender).ok().or(Some(existing));
            let _ = tx.send(Job::Imported { report, existing: fresh });
            ctx.request_repaint();
        });
    }

    // ------------------------------------------------------------------
    // Drawing the window
    // ------------------------------------------------------------------
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();

        egui::Panel::top("source").show(ui, |ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Source drawing").strong());
                let width = (ui.available_width() - 190.0).max(160.0);
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut self.source_path)
                        .hint_text("C:\\jobs\\source.dxf")
                        .desired_width(width),
                );
                if ui.add_enabled(!self.busy, egui::Button::new("Browse...")).clicked() {
                    if let Some(p) = pick_file() {
                        self.source_path = p;
                        self.start_read(&ctx);
                    }
                }
                let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if ui.add_enabled(!self.busy, egui::Button::new("Read")).clicked() || (enter && !self.busy) {
                    self.start_read(&ctx);
                }
            });
            let color = if self.status_is_error { Color32::from_rgb(230, 110, 100) } else { ui.visuals().weak_text_color() };
            ui.horizontal(|ui| {
                if self.busy {
                    ui.spinner();
                }
                ui.label(RichText::new(&self.status).color(color));
            });
            ui.add_space(6.0);
        });

        let Some(loaded) = self.loaded.clone() else {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.add_space(24.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new("No source drawing yet").size(18.0).strong());
                    ui.label("Type a path above, or press Browse..., then Read.");
                });
            });
            return;
        };

        // ---- left: the type list -------------------------------------
        egui::Panel::left("types").exact_size(210.0).resizable(false).show(ui, |ui| {
            ui.add_space(8.0);
            ui.label(RichText::new("TYPE").small().weak());
            for cat in Cat::ALL {
                let n = loaded.catalog.get(cat).len();
                let t = self.sel.count(cat);
                let text = if t > 0 { format!("{}  ({n})   [{t}]", cat.title()) } else { format!("{}  ({n})", cat.title()) };
                let selected = self.cat == cat;
                let button = egui::Button::new(RichText::new(text).size(13.5))
                    .selected(selected)
                    .frame(selected)
                    .min_size(Vec2::new(ui.available_width(), 30.0));
                if ui.add(button).clicked() {
                    self.cat = cat;
                }
            }
            ui.add_space(10.0);
            ui.separator();
            ui.label(RichText::new("[n] = ticked").small().weak());
        });

        // ---- bottom: options, import, last report ---------------------
        egui::Panel::bottom("actions").show(ui, |ui| {
            ui.add_space(6.0);
            ui.checkbox(&mut self.needs, "Also import what ticked items need (layers, linetypes, text styles, nested blocks)");
            ui.checkbox(&mut self.overwrite, "Overwrite layers, linetypes, text styles and dimension styles that already exist");
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let total = self.sel.total();
                let label = if total > 0 { format!("Import {total} ticked") } else { "Import ticked".to_string() };
                if ui.add_enabled(!self.busy && total > 0, egui::Button::new(RichText::new(label).strong()).min_size(Vec2::new(150.0, 28.0))).clicked() {
                    self.start_import(&ctx);
                }
                if ui.add_enabled(total > 0, egui::Button::new("Clear all ticks")).clicked() {
                    self.sel.clear();
                }
            });
            if !self.report.is_empty() {
                ui.add_space(4.0);
                egui::CollapsingHeader::new("Last import").default_open(true).show(ui, |ui| {
                    egui::ScrollArea::vertical().id_salt("report").max_height(130.0).show(ui, |ui| {
                        for line in &self.report {
                            let bad = line.contains("FAILED");
                            let text = RichText::new(line).monospace().small();
                            ui.label(if bad { text.color(Color32::from_rgb(230, 110, 100)) } else { text });
                        }
                    });
                });
            }
            ui.add_space(6.0);
        });

        // ---- centre: the chosen type ----------------------------------
        egui::CentralPanel::default().show(ui, |ui| {
            let cat = self.cat;
            let items = loaded.catalog.get(cat);
            let visible = self.sel.visible(cat, items, &self.existing, self.hide_existing);
            let ticked = self.sel.count(cat);

            ui.add_space(4.0);
            ui.label(
                RichText::new(format!("{}: {} shown of {}, {} ticked", cat.title(), visible.len(), items.len(), ticked)).strong(),
            );
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label("Filter");
                let filter = self.sel.filters.entry(cat.key()).or_default();
                ui.add(
                    egui::TextEdit::singleline(filter)
                        .hint_text("e.g. WALL*, TXT  (comma separated)")
                        .desired_width(f32::INFINITY),
                );
            });
            ui.horizontal(|ui| {
                if ui.button("Clear filter").clicked() {
                    self.sel.filters.remove(cat.key());
                }
                ui.separator();
                if ui.button("Select all").on_hover_text("Tick everything the filter shows").clicked() {
                    self.sel.bulk(cat, items, &visible, Bulk::All);
                }
                if ui.button("Select none").on_hover_text("Untick everything the filter shows").clicked() {
                    self.sel.bulk(cat, items, &visible, Bulk::None);
                }
                if ui.button("Invert").on_hover_text("Flip the ticks of everything the filter shows").clicked() {
                    self.sel.bulk(cat, items, &visible, Bulk::Invert);
                }
                ui.separator();
                ui.checkbox(&mut self.hide_existing, "Hide items already in the drawing");
            });
            ui.add_space(4.0);
            ui.separator();

            if items.is_empty() {
                ui.label(RichText::new("The source has none of these.").weak());
                return;
            }
            if visible.is_empty() {
                ui.label(RichText::new("Nothing matches the filter.").weak());
                return;
            }
            let row_h = ui.text_style_height(&egui::TextStyle::Body) + ui.spacing().item_spacing.y + 4.0;
            egui::ScrollArea::vertical().id_salt(("items", cat.key())).auto_shrink([false, false]).show_rows(ui, row_h, visible.len(), |ui, range| {
                for &i in &visible[range] {
                    let name = &items[i];
                    let exists = self.existing.has(cat, name);
                    let mut on = self.sel.is_on(cat, name);
                    let label = if exists { format!("{name}   (exists)") } else { name.clone() };
                    let text = if exists { RichText::new(label).weak() } else { RichText::new(label) };
                    if ui.checkbox(&mut on, text).changed() {
                        self.sel.set(cat, name, on);
                    }
                }
            });
        });
    }
}

/// A native "open file" dialog (Windows). Elsewhere the path is typed.
#[cfg(windows)]
fn pick_file() -> Option<String> {
    rfd::FileDialog::new()
        .set_title("Choose the source drawing")
        .add_filter("Drawings (DXF, DWG)", &["dxf", "dwg"])
        .pick_file()
        .map(|p| p.display().to_string())
}

#[cfg(not(windows))]
fn pick_file() -> Option<String> {
    None
}
