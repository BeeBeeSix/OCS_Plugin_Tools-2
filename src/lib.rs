//! Tools - a collection of tools for Open CAD Studio.
//!
//! Steal imports layers, linetypes, text styles, dimension styles and blocks from another
//! drawing into the drawing open in Open CAD Studio.
//!
//! Command `STEAL` (ribbon: Tools > Import > Steal) opens the Steal window. The window runs in the
//! plugin process, next to the CAD window, and writes into the drawing through the host's
//! validated table operations, as one undo step.

mod dvt;
mod model;
mod ui;
mod views;

use std::sync::{Arc, Mutex, OnceLock};

use ocs_plugin_api::export_plugin;
use ocs_plugin_api::host::{BuiltinPlugin, Handle, HostApi, PluginRequestSender};
use ocs_plugin_api::host::{EntityType, TableOperation};
use ocs_plugin_api::ipc::protocol::{PluginRequest, PluginResponse};
use ocs_plugin_api::manifest::{ApiVersion, PluginManifest};
use ocs_plugin_api::ribbon::{CadModule, IconKind, ModuleEvent, RibbonGroup, RibbonItem, ToolDef};

static MANIFEST: PluginManifest = PluginManifest {
    id: "com.beebeesix.ocs-plugin-tools",
    name: "Tools",
    version: env!("CARGO_PKG_VERSION"),
    description: "A collection of tools for Open CAD Studio.",
    api_version: ApiVersion::CURRENT,
    ribbon_order: 120,
    xdata_apps: &[],
    command_prefixes: &["STEAL", "DVT", "01", "02", "03", "04", "05", "06", "07", "08", "09", "ZE", "UV", "UW"],
};

/// Step-by-step log in the temp folder (%TEMP%\ocs-tools.log) so a silent failure can be diagnosed.
pub fn log(msg: &str) {
    use std::io::Write;
    static T0: OnceLock<std::time::Instant> = OnceLock::new();
    let t = T0.get_or_init(std::time::Instant::now).elapsed().as_secs_f32();
    let path = std::env::temp_dir().join("ocs-tools.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "[{t:>8.1}s] {msg}");
    }
}

pub fn log_path() -> String {
    std::env::temp_dir().join("ocs-tools.log").display().to_string()
}

// ----------------------------------------------------------------------
// The host connection used by the window and its worker threads
// ----------------------------------------------------------------------
/// The drawing tab Steal writes into: remembered from the last STEAL command.
#[derive(Clone)]
pub struct Target {
    pub sender: Arc<dyn PluginRequestSender>,
    pub tab_id: u64,
}

fn target_slot() -> &'static Mutex<Option<Target>> {
    static T: OnceLock<Mutex<Option<Target>>> = OnceLock::new();
    T.get_or_init(|| Mutex::new(None))
}

pub fn target() -> Option<Target> {
    target_slot().lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Called from `dispatch`: remember the active tab and a thread-safe request sender.
fn attach(host: &mut dyn HostApi) -> bool {
    let Some(sender) = host.plugin_request_sender() else { return false };
    let tab_id = host.tab_id();
    *target_slot().lock().unwrap_or_else(|e| e.into_inner()) = Some(Target { sender: Arc::from(sender), tab_id });
    true
}

/// [`model::Ops`] over the request sender: how the window thread edits the drawing.
pub struct SenderOps {
    pub sender: Arc<dyn PluginRequestSender>,
}

impl SenderOps {
    fn send(&self, req: PluginRequest) -> Result<PluginResponse, String> {
        match self.sender.request(req) {
            Ok(PluginResponse::Error(e)) => Err(e),
            Ok(other) => Ok(other),
            Err(e) => Err(e.0),
        }
    }
}

impl model::Ops for SenderOps {
    fn table_operation(&mut self, operation: TableOperation) -> Result<Handle, String> {
        match self.send(PluginRequest::TableOperation { operation })? {
            PluginResponse::TableResult(r) => r,
            other => Err(format!("unexpected reply: {other:?}")),
        }
    }
    fn add_entity(&mut self, entity: EntityType) -> Handle {
        match self.send(PluginRequest::AddEntity(entity)) {
            Ok(PluginResponse::Handle(h)) => h,
            _ => Handle::NULL,
        }
    }
    fn begin_undo(&mut self, label: &str) {
        let _ = self.send(PluginRequest::BeginUndoGroup { label: label.to_string() });
    }
    fn end_undo(&mut self) {
        let _ = self.send(PluginRequest::EndUndoGroup);
        let _ = self.send(PluginRequest::SetDirty);
        let _ = self.send(PluginRequest::BumpGeometry);
    }
}

/// Names already in the open drawing (a snapshot request to the host).
pub fn existing_names(sender: &Arc<dyn PluginRequestSender>) -> Result<model::Existing, String> {
    match sender.request(PluginRequest::DocumentSnapshot) {
        Ok(PluginResponse::Document(doc)) => Ok(model::Existing::of(&doc)),
        Ok(PluginResponse::Error(e)) => Err(e),
        Ok(other) => Err(format!("unexpected reply: {other:?}")),
        Err(e) => Err(e.0),
    }
}

// ----------------------------------------------------------------------
// The plugin
// ----------------------------------------------------------------------
struct ToolsPlugin;

impl BuiltinPlugin for ToolsPlugin {
    fn manifest(&self) -> &'static PluginManifest {
        &MANIFEST
    }

    fn ribbon(&self) -> Box<dyn CadModule> {
        Box::new(ToolsModule)
    }

    fn dispatch(&self, host: &mut dyn HostApi, cmd: &str) -> bool {
        if let Some(line) = views::host_command(cmd) {
            // One call per line. A command left running (UCS waits at its prompt) would take the
            // next line as answers to its prompts, so ESC clears it between lines.
            let mut first = true;
            for l in line.lines() {
                if !first {
                    host.execute_command("ESC");
                }
                first = false;
                if !host.execute_command(l) {
                    host.push_error(&format!("{cmd}: host did not run \"{l}\""));
                    break;
                }
            }
            if line.contains('\n') {
                // The last line may leave its command at a prompt (DVIEW waits for an option).
                host.execute_command("ESC");
            }
            return true;
        }
        if cmd.eq_ignore_ascii_case("DVT") {
            dvt::start(host);
            return true;
        }
        if !cmd.eq_ignore_ascii_case("STEAL") {
            return false;
        }
        log("--- STEAL started");
        if !attach(host) {
            host.push_error("Steal: the host gave the plugin no request channel, so it cannot edit the drawing.");
        }
        match ui::open() {
            Ok(()) => log("window shown"),
            Err(e) => {
                log(&format!("window failed: {e}"));
                host.push_error(&format!("Steal: {e}"));
            }
        }
        log("--- STEAL finished");
        true
    }
}

struct ToolsModule;

impl CadModule for ToolsModule {
    fn id(&self) -> &'static str {
        MANIFEST.id
    }

    fn title(&self) -> &'static str {
        "Tools"
    }

    fn ribbon_groups(&self) -> &[RibbonGroup] {
        static GROUPS: OnceLock<Vec<RibbonGroup>> = OnceLock::new();
        GROUPS.get_or_init(|| {
            vec![
                RibbonGroup {
                    title: "Import",
                    tools: vec![RibbonItem::LargeTool(ToolDef {
                        id: "STEAL",
                        label: "Steal",
                        icon: IconKind::Glyph("S"),
                        event: ModuleEvent::Command("STEAL".to_string()),
                    })],
                },
                RibbonGroup {
                    title: "View",
                    tools: vec![RibbonItem::LargeTool(ToolDef {
                        id: "DVT",
                        label: "DVT",
                        icon: IconKind::Glyph("D"),
                        event: ModuleEvent::Command("DVT".to_string()),
                    })],
                },
            ]
        })
    }
}

export_plugin!(ToolsPlugin);

#[cfg(test)]
mod tests;
