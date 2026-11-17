//! Tests: the engine against a mock host, and the real window driven headlessly
//! (rendered without a screen, buttons clicked and text typed with synthetic input).

use std::sync::{Arc, Mutex};

use eframe::egui;
use ocs_plugin_api::host::acadrust::Document;
use ocs_plugin_api::host::{EntityType, Handle, PluginRequestError, PluginRequestSender, TableOperation};
use ocs_plugin_api::ipc::protocol::{PluginRequest, PluginResponse};

use crate::model::*;
use crate::ui::{App, Loaded};

fn source() -> Document {
    let path = std::env::var("STEAL_TEST_SRC").unwrap_or_else(|_| "sample.dxf".to_string());
    read_source(&path).expect("source drawing reads")
}

/// A host that records what it is asked and enforces a few of the real host's rules.
#[derive(Default)]
struct Host {
    ops: Vec<String>,
    linetypes: std::collections::HashSet<String>,
    undo_open: bool,
    next: u64,
}

impl Host {
    fn table(&mut self, op: &TableOperation) -> Result<Handle, String> {
        let line = match op {
            TableOperation::DimStyleCreate { name, properties, .. } => {
                let v: serde_json::Value = serde_json::from_str(properties).unwrap();
                let keys: Vec<&String> = v.as_object().unwrap().keys().collect();
                let bad: Vec<&&String> = keys
                    .iter()
                    .filter(|k| k.as_str() == "handle" || k.as_str() == "name" || k.starts_with("xref") || k.ends_with("_handle"))
                    .collect();
                format!("DimStyleCreate {name} ({} fields; managed fields leaked: {:?})", keys.len(), bad)
            }
            TableOperation::LayerCreate { config } => format!("LayerCreate {} linetype={:?}", config.name, config.linetype),
            TableOperation::TextStyleCreate { config } => format!("TextStyleCreate {}", config.name),
            TableOperation::LinetypeCreate { name, .. } => format!("LinetypeCreate {name}"),
            TableOperation::LayoutCreate { name } => format!("LayoutCreate {name}"),
            TableOperation::LayoutSetPage { name, paper_size, rotation, scale } => format!("LayoutSetPage {name} size={paper_size:?} rot={rotation:?} scale={scale:?}"),
            TableOperation::BlockCreate { name, entities, .. } => format!("BlockCreate {name} from {} handles", entities.len()),
            other => format!("{other:?}").chars().take(110).collect(),
        };
        self.ops.push(line);
        match op {
            TableOperation::LinetypeCreate { name, .. } => {
                self.linetypes.insert(name.to_lowercase());
            }
            TableOperation::LayerCreate { config } => {
                if let Some(lt) = &config.linetype {
                    if !is_builtin_linetype(lt) && !self.linetypes.contains(&lt.to_lowercase()) {
                        return Err(format!("linetype {lt} not found"));
                    }
                }
            }
            TableOperation::DimStyleCreate { properties, .. } if properties.contains("\"dimblk_name\"") => {
                return Err("unknown or mistyped field `dimblk_name`".into());
            }
            _ => {}
        }
        self.next += 1;
        Ok(Handle::new(self.next))
    }
}

struct MockOps(Host);

impl Ops for MockOps {
    fn table_operation(&mut self, op: TableOperation) -> Result<Handle, String> {
        self.0.table(&op)
    }
    fn add_entity(&mut self, _e: EntityType) -> Handle {
        self.0.next += 1;
        Handle::new(self.0.next)
    }
    fn begin_undo(&mut self, _l: &str) {
        self.0.undo_open = true;
    }
    fn end_undo(&mut self) {
        self.0.undo_open = false;
    }
}

/// The same host, behind the request channel the window thread uses.
struct MockSender(Mutex<Host>);

impl PluginRequestSender for MockSender {
    fn request(&self, req: PluginRequest) -> Result<PluginResponse, PluginRequestError> {
        let mut h = self.0.lock().unwrap();
        Ok(match req {
            PluginRequest::TableOperation { operation } => PluginResponse::TableResult(h.table(&operation)),
            PluginRequest::AddEntity(_) => {
                h.next += 1;
                PluginResponse::Handle(Handle::new(h.next))
            }
            PluginRequest::BeginUndoGroup { .. } => {
                h.undo_open = true;
                PluginResponse::Ok
            }
            PluginRequest::EndUndoGroup => {
                h.undo_open = false;
                PluginResponse::Ok
            }
            PluginRequest::DocumentSnapshot => PluginResponse::Document(Box::new(Document::default())),
            _ => PluginResponse::Ok,
        })
    }
}

// ---- the selection model -----------------------------------------------
#[test]
fn selection_filter_all_none_invert() {
    let doc = source();
    let catalog = catalog_of(&doc);
    let existing = Existing::default();
    let items = catalog.get(Cat::TextStyles);
    assert!(items.len() > 10);
    let mut sel = Selection::default();

    sel.filters.insert(Cat::TextStyles.key(), "ARIAL*".into());
    let vis = sel.visible(Cat::TextStyles, items, &existing, false);
    assert!(!vis.is_empty() && vis.len() < items.len());
    assert!(vis.iter().all(|&i| items[i].to_lowercase().starts_with("arial")));

    sel.bulk(Cat::TextStyles, items, &vis, Bulk::All);
    assert_eq!(sel.count(Cat::TextStyles), vis.len());
    sel.bulk(Cat::TextStyles, items, &vis, Bulk::None);
    assert_eq!(sel.count(Cat::TextStyles), 0);
    sel.bulk(Cat::TextStyles, items, &vis, Bulk::All);
    sel.set(Cat::TextStyles, &items[vis[0]], false);
    sel.bulk(Cat::TextStyles, items, &vis, Bulk::Invert);
    assert_eq!(sel.count(Cat::TextStyles), 1, "invert flips only the filtered items");

    // items outside the filter are never touched by the bulk buttons
    sel.filters.insert(Cat::TextStyles.key(), String::new());
    let all = sel.visible(Cat::TextStyles, items, &existing, false);
    assert_eq!(all.len(), items.len());
    sel.bulk(Cat::TextStyles, items, &vis, Bulk::None);
    sel.set(Cat::TextStyles, "ZZZ-outside", true);
    assert_eq!(sel.count(Cat::TextStyles), 1);

    // hiding items that already exist
    let mut ex = Existing::default();
    ex.add(Cat::TextStyles, &items[0]);
    let shown = sel.visible(Cat::TextStyles, items, &ex, true);
    assert_eq!(shown.len(), items.len() - 1);

    // filters: plain text is a substring, wildcards match the whole name, commas add alternatives
    assert!(filter_match("txt", "MYTXT50"));
    assert!(!filter_match("DOOR*", "MYDOORS"));
    assert!(filter_match("WALLS, HW", "hw"));
}

// ---- the import --------------------------------------------------------
#[test]
fn import_order_and_dependencies() {
    let doc = source();
    let catalog = catalog_of(&doc);
    let mut sel = Selection::default();
    for (cat, name) in [(Cat::Layers, "WALLS"), (Cat::Layers, "HW"), (Cat::TextStyles, "ARIALX"), (Cat::Blocks, "BOLT"), (Cat::DimStyles, "MYDIM"), (Cat::Layouts, "Layout1")] {
        assert!(catalog.get(cat).iter().any(|n| n == name), "{name} in the test source");
        sel.set(cat, name, true);
    }
    let mut ops = MockOps(Host { linetypes: ["continuous".to_string()].into_iter().collect(), ..Default::default() });
    let mut existing = Existing::default();
    let report = import_selected(&mut ops, &doc, &mut existing, &sel.choice(&catalog, false, true));
    for l in &report.lines {
        println!("{l}");
    }
    for n in &report.notes {
        println!("note: {n}");
    }
    let h = &ops.0;
    assert!(!h.undo_open, "undo group left open");
    let pos = |needle: &str| h.ops.iter().position(|o| o.starts_with(needle)).unwrap_or_else(|| panic!("missing {needle}: {:?}", h.ops));
    assert!(pos("LinetypeCreate CENTER") < pos("LayerCreate WALLS"), "linetype before the layer that uses it");
    assert!(pos("TextStyleCreate ARIALX") < pos("BlockCreate BOLT"));
    assert!(pos("BlockCreate NUT") < pos("BlockCreate BOLT"), "nested block first");
    assert!(h.ops.iter().any(|o| o.starts_with("DimStyleCreate MYDIM")));
    assert!(h.ops.iter().any(|o| o == "LayoutCreate Layout1"), "{:?}", h.ops);
    println!("layout ops: {:?}", h.ops.iter().filter(|o| o.starts_with("Layout")).collect::<Vec<_>>());
    assert!(!h.ops.iter().any(|o| o.contains("leaked: [\"")), "host-managed fields must not be sent");
    assert_eq!(report.failed, 0, "{:?}", report.lines);
    assert!(existing.has(Cat::Blocks, "BOLT") && existing.has(Cat::Blocks, "NUT"));

    // a second run creates nothing: everything now exists
    let before = ops.0.ops.len();
    let again = import_selected(&mut ops, &doc, &mut existing, &sel.choice(&catalog, false, true));
    assert_eq!(ops.0.ops.len(), before);
    assert!(again.lines.iter().all(|l| l.contains("already in the drawing")), "{:?}", again.lines);
}

// ---- the window, driven headlessly -------------------------------------
struct Screen {
    ctx: egui::Context,
    t: f64,
}

fn collect(shape: &egui::epaint::Shape, out: &mut Vec<(String, egui::Rect)>) {
    use egui::epaint::Shape;
    match shape {
        Shape::Text(t) => out.push((t.galley.text().to_string(), egui::Rect::from_min_size(t.pos, t.galley.size()))),
        Shape::Vec(v) => v.iter().for_each(|s| collect(s, out)),
        _ => {}
    }
}

impl Screen {
    fn new() -> Self {
        Screen { ctx: egui::Context::default(), t: 0.0 }
    }

    /// Run one frame and return every piece of text drawn, with its position.
    fn frame(&mut self, app: &mut App, events: Vec<egui::Event>) -> Vec<(String, egui::Rect)> {
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(940.0, 680.0)));
        input.time = Some(self.t);
        self.t += 0.05;
        input.events = events;
        let mut out = self.ctx.run_ui(input, |ui| {
            app.poll(&ui.ctx().clone());
            app.ui(ui);
        });
        out.textures_delta.clear(); // no GPU here: nothing to upload
        let mut texts = Vec::new();
        for s in &out.shapes {
            collect(&s.shape, &mut texts);
        }
        texts
    }

    fn settle(&mut self, app: &mut App) -> Vec<(String, egui::Rect)> {
        self.frame(app, vec![]);
        self.frame(app, vec![])
    }

    /// Click the widget whose text is exactly `label`.
    fn click(&mut self, app: &mut App, label: &str) {
        println!("click {label:?}");
        let texts = self.settle(app);
        let rect = texts.iter().find(|(t, _)| t == label).unwrap_or_else(|| panic!("no widget {label:?} on screen; have {:?}", texts.iter().map(|x| &x.0).collect::<Vec<_>>())).1;
        let pos = rect.center();
        self.frame(app, vec![egui::Event::PointerMoved(pos)]);
        let press = |pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        self.frame(app, vec![press(true)]);
        self.frame(app, vec![press(false)]);
        self.settle(app);
    }

    fn text_starting(&mut self, app: &mut App, prefix: &str) -> Option<String> {
        self.settle(app).into_iter().map(|x| x.0).find(|t| t.starts_with(prefix))
    }
}

fn wait_idle(screen: &mut Screen, app: &mut App) {
    for _ in 0..200 {
        screen.frame(app, vec![]);
        if !app.busy {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    panic!("the background job never finished");
}

#[test]
fn window_flow_headless() {
    let doc = source();
    let catalog = catalog_of(&doc);
    let mut app = App::new(None);
    let mut screen = Screen::new();

    // before anything is read the window explains what to do
    let texts = screen.settle(&mut app);
    assert!(texts.iter().any(|(t, _)| t.contains("No source drawing yet")));
    assert!(texts.iter().any(|(t, _)| t == "Read"));

    // a source arrives: type list, tools and items appear
    app.set_loaded(Loaded { file: "source.dxf".into(), doc, catalog }, Some(Existing::default()));
    let texts = screen.settle(&mut app);
    let have = |t: &str| texts.iter().any(|(x, _)| x == t);
    for t in ["Select all", "Select none", "Invert", "Clear filter", "Import ticked"] {
        assert!(have(t), "missing {t}");
    }
    assert!(texts.iter().any(|(t, _)| t.starts_with("Layers  (")), "type list shows Layers");
    assert!(texts.iter().any(|(t, _)| t.starts_with("Blocks  (")));
    assert!(texts.iter().any(|(t, _)| t == "WALLS"), "layer items are listed");

    // switch the type by clicking it in the left list
    let nav = texts.iter().find(|(t, _)| t.starts_with("Text styles  (")).unwrap().0.clone();
    screen.click(&mut app, &nav);
    let header = screen.text_starting(&mut app, "Text styles:").expect("centre shows the chosen type");
    println!("{header}");
    assert!(header.ends_with("0 ticked"));
    let total = app.loaded.as_ref().unwrap().catalog.text_styles.len();
    assert!(header.contains(&format!("{total} shown of {total}")));

    // type in the filter box (click it, then type)
    screen.click(&mut app, "e.g. WALL*, TXT  (comma separated)");
    screen.frame(&mut app, vec![egui::Event::Text("ARIAL*".into())]);
    screen.settle(&mut app);
    let header = screen.text_starting(&mut app, "Text styles:").unwrap();
    println!("{header}");
    assert!(!header.contains(&format!("{total} shown")), "the list shrinks while typing: {header}");
    let shown: usize = header.split("shown").next().unwrap().rsplit(' ').nth(1).unwrap().parse().unwrap();
    assert!(shown >= 1);

    // Select all / none / Invert act on what the filter shows
    screen.click(&mut app, "Select all");
    assert_eq!(app.sel.count(cat_textstyles()), shown);
    assert!(screen.text_starting(&mut app, "Text styles:").unwrap().ends_with(&format!("{shown} ticked")));
    screen.click(&mut app, "Select none");
    assert_eq!(app.sel.count(cat_textstyles()), 0);
    screen.click(&mut app, "Invert");
    assert_eq!(app.sel.count(cat_textstyles()), shown, "invert of nothing ticks everything shown");
    screen.click(&mut app, "Invert");
    assert_eq!(app.sel.count(cat_textstyles()), 0);

    // tick one item by clicking its checkbox
    screen.click(&mut app, "ARIALX");
    assert_eq!(app.sel.count(cat_textstyles()), 1);
    assert!(app.sel.is_on(cat_textstyles(), "ARIALX"));
    // the type list shows the tick count
    assert!(screen.settle(&mut app).iter().any(|(t, _)| t.starts_with("Text styles  (") && t.ends_with("[1]")));

    // Clear filter brings the whole list back; the tick stays
    screen.click(&mut app, "Clear filter");
    let header = screen.text_starting(&mut app, "Text styles:").unwrap();
    assert!(header.contains(&format!("{total} shown of {total}")) && header.ends_with("1 ticked"), "{header}");

    // layers: select all of them through the real buttons
    let layers_nav = screen_text(&mut screen, &mut app, "Layers  (");
    screen.click(&mut app, &layers_nav);
    screen.click(&mut app, "Select all");
    let nlayers = app.loaded.as_ref().unwrap().catalog.layers.len();
    assert_eq!(app.sel.count(Cat::Layers), nlayers);
    screen.click(&mut app, "Select none");

    // ---- import through the request channel, on a worker thread ----------
    app.sel.set(Cat::Layers, "WALLS", true);
    app.sel.set(Cat::Blocks, "BOLT", true);
    let host = Arc::new(MockSender(Mutex::new(Host { linetypes: ["continuous".to_string()].into_iter().collect(), ..Default::default() })));
    *crate::target_slot().lock().unwrap() = Some(crate::Target { sender: host.clone(), tab_id: 1 });
    let button = screen_text(&mut screen, &mut app, "Import ");
    assert!(button.starts_with("Import ") && button.contains("ticked"), "{button}");
    screen.click(&mut app, &button);
    wait_idle(&mut screen, &mut app);
    let h = host.0.lock().unwrap();
    println!("host saw: {:?}", h.ops);
    assert!(h.ops.iter().any(|o| o.starts_with("LayerCreate WALLS")));
    assert!(h.ops.iter().any(|o| o.starts_with("BlockCreate BOLT")));
    assert!(!h.undo_open, "undo step closed");
    drop(h);
    assert_eq!(app.sel.total(), 0, "ticks are cleared after the import");
    assert!(app.report.iter().any(|l| l.contains("BOLT: created")), "{:?}", app.report);
    assert!(app.status.starts_with("Done"), "{}", app.status);
    // the report appears in the window
    assert!(screen.settle(&mut app).iter().any(|(t, _)| t.contains("Last import")));
    *crate::target_slot().lock().unwrap() = None;
}

fn cat_textstyles() -> Cat {
    Cat::TextStyles
}

fn screen_text(screen: &mut Screen, app: &mut App, prefix: &str) -> String {
    screen.text_starting(app, prefix).unwrap_or_else(|| panic!("no text starting {prefix:?}"))
}
