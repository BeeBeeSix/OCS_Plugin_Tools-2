//! The engine behind Steal: what a drawing contains, which items are picked, the
//! dependency-ordered import plan and the table operations. Nothing here touches the
//! window or the host directly; the host is reached through [`Ops`].

use std::collections::{HashMap, HashSet};

use ocs_plugin_api::host::acadrust::objects::{Layout, ObjectType};
use ocs_plugin_api::host::acadrust::{Document, DwgReader, DxfReader};
use ocs_plugin_api::host::{
    EntityType, Handle, LayerConfig, TableOperation, TextStyleConfig,
};

/// What the import needs from the host. The window thread implements it with a
/// request sender; tests implement it with a mock.
pub trait Ops {
    fn table_operation(&mut self, op: TableOperation) -> Result<Handle, String>;
    fn add_entity(&mut self, entity: EntityType) -> Handle;
    fn begin_undo(&mut self, label: &str);
    fn end_undo(&mut self);
}

// ----------------------------------------------------------------------
// Catalogue of what the source and the open drawing contain
// ----------------------------------------------------------------------
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Cat {
    Layers,
    Linetypes,
    TextStyles,
    DimStyles,
    Blocks,
    Layouts,
}

impl Cat {
    pub const ALL: [Cat; 6] =
        [Cat::Layers, Cat::Linetypes, Cat::TextStyles, Cat::DimStyles, Cat::Blocks, Cat::Layouts];

    pub fn key(self) -> &'static str {
        match self {
            Cat::Layers => "layers",
            Cat::Linetypes => "linetypes",
            Cat::TextStyles => "textstyles",
            Cat::DimStyles => "dimstyles",
            Cat::Blocks => "blocks",
            Cat::Layouts => "layouts",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Cat::Layers => "Layers",
            Cat::Linetypes => "Linetypes",
            Cat::TextStyles => "Text styles",
            Cat::DimStyles => "Dimension styles",
            Cat::Blocks => "Blocks",
            Cat::Layouts => "Layouts",
        }
    }
}

#[derive(Default, Clone)]
pub struct Catalog {
    pub layers: Vec<String>,
    pub linetypes: Vec<String>,
    pub text_styles: Vec<String>,
    pub dim_styles: Vec<String>,
    pub blocks: Vec<String>,
    pub layouts: Vec<String>,
}

impl Catalog {
    pub fn get(&self, cat: Cat) -> &Vec<String> {
        match cat {
            Cat::Layers => &self.layers,
            Cat::Linetypes => &self.linetypes,
            Cat::TextStyles => &self.text_styles,
            Cat::DimStyles => &self.dim_styles,
            Cat::Blocks => &self.blocks,
            Cat::Layouts => &self.layouts,
        }
    }
}

pub fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort_by_key(|s| s.to_lowercase());
    v
}

pub fn is_builtin_linetype(name: &str) -> bool {
    ["continuous", "bylayer", "byblock"].contains(&name.to_lowercase().as_str())
}

/// What the source drawing offers.
pub fn catalog_of(doc: &Document) -> Catalog {
    Catalog {
        layers: sorted(doc.layers.names().map(str::to_owned).collect()),
        linetypes: sorted(
            doc.line_types
                .iter()
                .filter(|l| !is_builtin_linetype(&l.name) && !l.xref_dependent)
                .map(|l| l.name.clone())
                .collect(),
        ),
        text_styles: sorted(
            doc.text_styles
                .iter()
                .filter(|s| !s.name.is_empty() && !s.xref_dependent && !s.is_shape_file)
                .map(|s| s.name.clone())
                .collect(),
        ),
        dim_styles: sorted(
            doc.dim_styles.iter().filter(|d| !d.xref_dependent).map(|d| d.name.clone()).collect(),
        ),
        blocks: sorted(
            doc.block_records
                .iter()
                .filter(|b| {
                    !b.name.starts_with('*')
                        && !b.flags.anonymous
                        && !b.flags.is_xref
                        && !b.flags.is_xref_overlay
                })
                .map(|b| b.name.clone())
                .collect(),
        ),
        layouts: source_layouts(doc).into_iter().map(|l| l.name.clone()).collect(),
    }
}

/// The paper-space layouts of a drawing, in tab order (the Model tab is not a layout here).
pub fn source_layouts(doc: &Document) -> Vec<&Layout> {
    let mut v: Vec<&Layout> = doc
        .objects
        .values()
        .filter_map(|o| match o {
            ObjectType::Layout(l) if !l.name.eq_ignore_ascii_case("model") => Some(l),
            _ => None,
        })
        .collect();
    v.sort_by(|a, b| a.tab_order.cmp(&b.tab_order).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    v
}

/// Lower-case names already in the open drawing, per category.
#[derive(Default, Clone)]
pub struct Existing {
    sets: HashMap<&'static str, HashSet<String>>,
}

impl Existing {
    pub fn of(doc: &Document) -> Self {
        let low = |it: &mut dyn Iterator<Item = &str>| it.map(|s| s.to_lowercase()).collect::<HashSet<_>>();
        let mut sets = HashMap::new();
        sets.insert("layers", low(&mut doc.layers.names()));
        sets.insert("linetypes", low(&mut doc.line_types.names()));
        sets.insert("textstyles", low(&mut doc.text_styles.names()));
        sets.insert("dimstyles", low(&mut doc.dim_styles.names()));
        sets.insert("blocks", low(&mut doc.block_records.names()));
        sets.insert(
            "layouts",
            source_layouts(doc).into_iter().map(|l| l.name.to_lowercase()).collect::<HashSet<_>>(),
        );
        Existing { sets }
    }

    pub fn has(&self, cat: Cat, name: &str) -> bool {
        self.sets.get(cat.key()).map_or(false, |s| s.contains(&name.to_lowercase()))
    }

    pub fn add(&mut self, cat: Cat, name: &str) {
        self.sets.entry(cat.key()).or_default().insert(name.to_lowercase());
    }
}

// ----------------------------------------------------------------------
// Reading the source
// ----------------------------------------------------------------------
pub fn read_source(path: &str) -> Result<Document, String> {
    let lower = path.to_lowercase();
    if lower.ends_with(".dwg") {
        let mut reader = DwgReader::from_file(path).map_err(|e| e.to_string())?;
        reader.read().map_err(|e| e.to_string())
    } else {
        DxfReader::from_file(path)
            .map_err(|e| e.to_string())?
            .read()
            .map_err(|e| e.to_string())
    }
}

// ----------------------------------------------------------------------
// Pattern matching: comma separated, * and ? wildcards, case-insensitive
// ----------------------------------------------------------------------
fn glob(pattern: &[char], text: &[char]) -> bool {
    match pattern.split_first() {
        None => text.is_empty(),
        Some(('*', rest)) => (0..=text.len()).any(|i| glob(rest, &text[i..])),
        Some(('?', rest)) => !text.is_empty() && glob(rest, &text[1..]),
        Some((c, rest)) => text.first() == Some(c) && glob(rest, &text[1..]),
    }
}


pub struct Choice {
    pub names: HashMap<&'static str, Vec<String>>, // explicit picks per category
    pub overwrite: bool,
    pub needs: bool,
}


/// Filter text: comma separated; an entry with * or ? is a wildcard pattern for the whole
/// name, any other entry matches names that contain it. Empty matches everything.
pub fn filter_match(filter: &str, name: &str) -> bool {
    let alts: Vec<String> = filter.split(',').map(|a| a.trim().to_lowercase()).filter(|a| !a.is_empty()).collect();
    if alts.is_empty() {
        return true;
    }
    let low = name.to_lowercase();
    let chars: Vec<char> = low.chars().collect();
    alts.iter().any(|a| {
        if a.contains('*') || a.contains('?') {
            glob(&a.chars().collect::<Vec<_>>(), &chars)
        } else {
            low.contains(a.as_str())
        }
    })
}


// ----------------------------------------------------------------------
// Planning: what to create, in dependency order
// ----------------------------------------------------------------------
#[derive(Default)]
struct Ordered {
    seen: HashSet<String>,
    items: Vec<String>,
}

impl Ordered {
    fn push(&mut self, name: &str) {
        if self.seen.insert(name.to_lowercase()) {
            self.items.push(name.to_string());
        }
    }
}

#[derive(Default)]
pub struct Plan {
    linetypes: Ordered,
    text_styles: Ordered,
    layers: Ordered,
    dim_styles: Ordered,
    blocks: Ordered, // post-order: nested blocks first
    layouts: Ordered,
    explicit: HashSet<(&'static str, String)>,
}

fn visit_block(src: &Document, existing: &Existing, name: &str, plan: &mut Plan, visiting: &mut HashSet<String>) {
    let low = name.to_lowercase();
    if plan.blocks.seen.contains(&low) || !visiting.insert(low) {
        return;
    }
    for e in src.entities_in_block(name) {
        let c = e.common();
        plan.layers.push(&c.layer);
        if !c.linetype.is_empty() && !is_builtin_linetype(&c.linetype) {
            plan.linetypes.push(&c.linetype);
        }
        match e {
            EntityType::Insert(i) => {
                if !existing.has(Cat::Blocks, &i.block_name) && src.block_records.contains(&i.block_name) {
                    visit_block(src, existing, &i.block_name, plan, visiting);
                }
            }
            EntityType::Text(t) => plan.text_styles.push(&t.style),
            EntityType::MText(m) => plan.text_styles.push(&m.style),
            _ => {}
        }
    }
    plan.blocks.push(name);
}

pub fn make_plan(src: &Document, existing: &Existing, choice: &Choice) -> Plan {
    let mut plan = Plan::default();
    let picked = |c: Cat| choice.names.get(c.key()).cloned().unwrap_or_default();

    for name in picked(Cat::Linetypes) {
        plan.explicit.insert(("linetypes", name.to_lowercase()));
        plan.linetypes.push(&name);
    }
    for name in picked(Cat::TextStyles) {
        plan.explicit.insert(("textstyles", name.to_lowercase()));
        plan.text_styles.push(&name);
    }
    for name in picked(Cat::Layers) {
        plan.explicit.insert(("layers", name.to_lowercase()));
        plan.layers.push(&name);
    }
    for name in picked(Cat::DimStyles) {
        plan.explicit.insert(("dimstyles", name.to_lowercase()));
        plan.dim_styles.push(&name);
    }
    for name in picked(Cat::Layouts) {
        plan.explicit.insert(("layouts", name.to_lowercase()));
        plan.layouts.push(&name);
    }
    let mut visiting = HashSet::new();
    for name in picked(Cat::Blocks) {
        plan.explicit.insert(("blocks", name.to_lowercase()));
        if choice.needs {
            visit_block(src, existing, &name, &mut plan, &mut visiting);
        } else {
            plan.blocks.push(&name);
        }
    }
    if choice.needs {
        let layer_names = plan.layers.items.clone();
        for l in layer_names {
            if let Some(layer) = src.layers.get(&l) {
                if !layer.line_type.is_empty() && !is_builtin_linetype(&layer.line_type) {
                    plan.linetypes.push(&layer.line_type);
                }
            }
        }
        let dim_names = plan.dim_styles.items.clone();
        for d in dim_names {
            if let Some(ds) = src.dim_styles.get(&d) {
                if !ds.dimtxsty.is_empty() {
                    plan.text_styles.push(&ds.dimtxsty);
                }
            }
        }
    }
    plan
}

// ----------------------------------------------------------------------
// Doing it
// ----------------------------------------------------------------------
pub enum Outcome {
    Created,
    Modified,
    Skipped(&'static str),
    Failed(String),
}

impl Outcome {
    fn text(&self) -> String {
        match self {
            Outcome::Created => "created".into(),
            Outcome::Modified => "overwritten".into(),
            Outcome::Skipped(why) => format!("skipped ({why})"),
            Outcome::Failed(e) => format!("FAILED - {e}"),
        }
    }
}

fn some_nonempty(s: &str) -> Option<String> {
    if s.trim().is_empty() { None } else { Some(s.to_string()) }
}

fn do_linetype(ops: &mut dyn Ops, src: &Document, name: &str, exists: bool) -> Outcome {
    let Some(lt) = src.line_types.get(name) else { return Outcome::Failed("not in the source".into()) };
    if lt.elements.iter().any(|e| e.complex.is_some()) {
        return Outcome::Failed("complex linetype (with shapes or text) - the host can only create simple ones".into());
    }
    let pattern: Vec<f64> = lt.elements.iter().map(|e| e.length).collect();
    let op = if exists {
        TableOperation::LinetypeModify {
            name: lt.name.clone(),
            description: Some(lt.description.clone()),
            pattern: Some(pattern),
        }
    } else {
        TableOperation::LinetypeCreate { name: lt.name.clone(), description: lt.description.clone(), pattern }
    };
    match ops.table_operation(op) {
        Ok(_) => if exists { Outcome::Modified } else { Outcome::Created },
        Err(e) => Outcome::Failed(e),
    }
}

fn do_text_style(ops: &mut dyn Ops, src: &Document, name: &str, exists: bool) -> Outcome {
    let Some(s) = src.text_styles.get(name) else { return Outcome::Failed("not in the source".into()) };
    let config = TextStyleConfig {
        name: s.name.clone(),
        height: Some(s.height),
        width_factor: Some(s.width_factor),
        oblique_angle: Some(s.oblique_angle),
        font_file: some_nonempty(&s.font_file),
        big_font_file: some_nonempty(&s.big_font_file),
        true_type_font: some_nonempty(&s.true_type_font),
        backward: Some(s.flags.backward),
        upside_down: Some(s.flags.upside_down),
        vertical: Some(s.is_vertical),
        annotative: Some(s.annotative),
    };
    let op = if exists { TableOperation::TextStyleModify { config } } else { TableOperation::TextStyleCreate { config } };
    match ops.table_operation(op) {
        Ok(_) => if exists { Outcome::Modified } else { Outcome::Created },
        Err(e) => Outcome::Failed(e),
    }
}

fn do_layer(ops: &mut dyn Ops, src: &Document, name: &str, exists: bool) -> Outcome {
    let Some(l) = src.layers.get(name) else { return Outcome::Failed("not in the source".into()) };
    let config = LayerConfig {
        name: l.name.clone(),
        color: Some(l.color.clone()),
        linetype: some_nonempty(&l.line_type),
        lineweight: Some(l.line_weight.clone()),
        off: Some(l.is_off()),
        frozen: Some(l.is_frozen()),
        locked: Some(l.is_locked()),
        plottable: Some(l.is_plottable),
        transparency: Some(l.transparency.clone()),
        description: some_nonempty(&l.description),
    };
    let op = if exists { TableOperation::LayerModify { config } } else { TableOperation::LayerCreate { config } };
    match ops.table_operation(op) {
        Ok(_) => if exists { Outcome::Modified } else { Outcome::Created },
        Err(e) => Outcome::Failed(e),
    }
}

/// Fields the host manages itself (handles, xref links, the name).
pub fn host_managed(key: &str) -> bool {
    key == "handle"
        || key == "name"
        || key.starts_with("xref")
        || key.ends_with("_handle")
        || ["dimblk", "dimblk1", "dimblk2", "dimldrblk"].contains(&key)
}

fn do_dim_style(ops: &mut dyn Ops, src: &Document, name: &str, exists: bool, notes: &mut Vec<String>) -> Outcome {
    let Some(d) = src.dim_styles.get(name) else { return Outcome::Failed("not in the source".into()) };
    let mut value = match serde_json::to_value(d) {
        Ok(v) => v,
        Err(e) => return Outcome::Failed(format!("cannot read the style: {e}")),
    };
    let Some(map) = value.as_object_mut() else { return Outcome::Failed("unexpected style shape".into()) };
    map.retain(|k, _| !host_managed(k));

    // The host refuses fields it does not accept and says which; drop each and retry.
    let mut dropped: Vec<String> = Vec::new();
    for _ in 0..40 {
        let properties = serde_json::Value::Object(map.clone()).to_string();
        let op = if exists {
            TableOperation::DimStyleModify { name: d.name.clone(), properties }
        } else {
            TableOperation::DimStyleCreate { name: d.name.clone(), copy_from: None, properties }
        };
        match ops.table_operation(op) {
            Ok(_) => {
                if !dropped.is_empty() {
                    notes.push(format!(
                        "dimension style {name}: the host refused {} field(s), imported without: {}",
                        dropped.len(),
                        dropped.join(", ")
                    ));
                }
                return if exists { Outcome::Modified } else { Outcome::Created };
            }
            Err(e) => {
                let culprit = map.keys().filter(|k| k.len() >= 4 && e.contains(k.as_str())).max_by_key(|k| k.len()).cloned();
                match culprit {
                    Some(k) => {
                        map.remove(&k);
                        dropped.push(k);
                    }
                    None => return Outcome::Failed(e),
                }
            }
        }
    }
    Outcome::Failed("too many refused fields".into())
}

/// Layouts: name, sheet size, rotation and plot scale. (The host API has no way to copy what is
/// drawn on a layout, so the new layout starts with the host's default sheet viewport.)
fn do_layout(ops: &mut dyn Ops, src: &Document, name: &str, exists: bool) -> Outcome {
    let Some(l) = source_layouts(src).into_iter().find(|l| l.name.eq_ignore_ascii_case(name)) else {
        return Outcome::Failed("not in the source".into());
    };
    if !exists {
        if let Err(e) = ops.table_operation(TableOperation::LayoutCreate { name: l.name.clone() }) {
            return Outcome::Failed(e);
        }
    }
    // sheet size in millimetres (DXF stores it in the layout's paper units)
    let mm = match l.plot_paper_units {
        0 => Some(25.4),
        1 => Some(1.0),
        _ => None,
    };
    let paper_size = mm.filter(|_| l.paper_width > 0.0 && l.paper_height > 0.0).map(|k| [l.paper_width * k, l.paper_height * k]);
    let rotation = match l.plot_rotation {
        0 => Some(0),
        1 => Some(90),
        2 => Some(180),
        3 => Some(270),
        90 | 180 | 270 => Some(l.plot_rotation as u16),
        _ => None,
    };
    let scale = if l.plot_scale_numerator > 0.0 && l.plot_scale_denominator > 0.0 {
        Some([l.plot_scale_numerator, l.plot_scale_denominator])
    } else {
        None
    };
    if paper_size.is_some() || rotation.is_some() || scale.is_some() {
        if let Err(e) = ops.table_operation(TableOperation::LayoutSetPage { name: l.name.clone(), paper_size, rotation, scale }) {
            let what = if exists { "kept" } else { "created" };
            return Outcome::Failed(format!("the layout was {what}, but its sheet setup was refused: {e}"));
        }
    }
    if exists { Outcome::Modified } else { Outcome::Created }
}

fn do_block(ops: &mut dyn Ops, src: &Document, name: &str) -> Outcome {
    let Some(rec) = src.block_records.get(name) else { return Outcome::Failed("not in the source".into()) };
    let entities: Vec<EntityType> = src.entities_in_block(name).cloned().collect();
    if entities.is_empty() {
        return Outcome::Failed("the block has no entities".into());
    }
    // 1. place a copy of each member in model space, at its block-space position
    let total = entities.len();
    let mut handles: Vec<Handle> = Vec::new();
    for mut e in entities {
        {
            let c = e.common_mut();
            c.handle = Handle::NULL;
            c.owner_handle = Handle::NULL;
            c.layer_handle = None;
            c.linetype_handle = None;
            c.reactors.clear();
            c.xdictionary_handle = None;
        }
        let h = ops.add_entity(e);
        if h != Handle::NULL {
            handles.push(h);
        }
    }
    if handles.is_empty() {
        return Outcome::Failed("the host refused every member entity".into());
    }
    let added = handles.len();
    // 2. define the block from them (origin 0,0,0) and erase the temporary copies
    let created = ops.table_operation(TableOperation::BlockCreate {
        name: rec.name.clone(),
        entities: handles,
        base_point: [0.0, 0.0, 0.0],
        erase_originals: true,
        description: some_nonempty(&rec.description),
    });
    if let Err(e) = created {
        return Outcome::Failed(e);
    }
    // 3. keep the source's explodable / uniform-scale settings
    if !rec.explodable || rec.scale_uniformly {
        let _ = ops.table_operation(TableOperation::BlockModify {
            name: rec.name.clone(),
            description: None,
            explodable: Some(rec.explodable),
            scale_uniformly: Some(rec.scale_uniformly),
        });
    }
    if added < total {
        Outcome::Failed(format!("created, but the host refused {} of {} member entities", total - added, total))
    } else {
        Outcome::Created
    }
}


/// `Some(outcome)` when nothing needs doing; `None` means go and do it.
pub fn decide(exists: bool, explicit: bool, overwrite: bool) -> Option<Outcome> {
    match (exists, explicit, overwrite) {
        (false, _, _) => None,
        (true, true, true) => None,
        (true, true, false) => Some(Outcome::Skipped("already in the drawing")),
        (true, false, _) => Some(Outcome::Skipped("already in the drawing (dependency)")),
    }
}


// ----------------------------------------------------------------------
// What is picked: per-type ticks and filters
// ----------------------------------------------------------------------
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Bulk {
    All,
    None,
    Invert,
}

/// Ticks (lower-case names) and filter text per type.
#[derive(Default)]
pub struct Selection {
    selected: HashMap<&'static str, HashSet<String>>,
    pub filters: HashMap<&'static str, String>,
}

impl Selection {
    pub fn is_on(&self, cat: Cat, name: &str) -> bool {
        self.selected.get(cat.key()).map_or(false, |s| s.contains(&name.to_lowercase()))
    }

    pub fn set(&mut self, cat: Cat, name: &str, on: bool) {
        let set = self.selected.entry(cat.key()).or_default();
        if on {
            set.insert(name.to_lowercase());
        } else {
            set.remove(&name.to_lowercase());
        }
    }

    pub fn count(&self, cat: Cat) -> usize {
        self.selected.get(cat.key()).map_or(0, |s| s.len())
    }

    pub fn total(&self) -> usize {
        Cat::ALL.into_iter().map(|c| self.count(c)).sum()
    }

    pub fn clear(&mut self) {
        self.selected.clear();
    }

    pub fn filter(&self, cat: Cat) -> &str {
        self.filters.get(cat.key()).map_or("", |s| s.as_str())
    }

    /// Indices of `items` that pass the type's filter (and are not hidden).
    pub fn visible(&self, cat: Cat, items: &[String], existing: &Existing, hide_existing: bool) -> Vec<usize> {
        let filter = self.filter(cat);
        (0..items.len())
            .filter(|&i| filter_match(filter, &items[i]) && !(hide_existing && existing.has(cat, &items[i])))
            .collect()
    }

    /// All / None / Invert on the given (visible) items only.
    pub fn bulk(&mut self, cat: Cat, items: &[String], visible: &[usize], mode: Bulk) {
        for &i in visible {
            let on = match mode {
                Bulk::All => true,
                Bulk::None => false,
                Bulk::Invert => !self.is_on(cat, &items[i]),
            };
            self.set(cat, &items[i], on);
        }
    }

    /// The picked names, in catalogue order.
    pub fn choice(&self, catalog: &Catalog, overwrite: bool, needs: bool) -> Choice {
        let mut names: HashMap<&'static str, Vec<String>> = HashMap::new();
        for c in Cat::ALL {
            names.insert(c.key(), catalog.get(c).iter().filter(|n| self.is_on(c, n)).cloned().collect());
        }
        Choice { names, overwrite, needs }
    }
}

// ----------------------------------------------------------------------
// Running an import
// ----------------------------------------------------------------------
#[derive(Default, Clone)]
pub struct Report {
    pub lines: Vec<String>,
    pub notes: Vec<String>,
    pub ok: usize,
    pub failed: usize,
}

impl Report {
    pub fn summary(&self) -> String {
        format!(
            "Done - {} item(s) imported, {} failed. One Undo reverts the whole import.",
            self.ok, self.failed
        )
    }
}

fn run_cat(
    report: &mut Report,
    existing: &mut Existing,
    plan: &Plan,
    overwrite: bool,
    cat: Cat,
    names: &[String],
    mut f: impl FnMut(&str, bool, &mut Vec<String>) -> Outcome,
) {
    for name in names {
        let exists = existing.has(cat, name);
        let explicit = plan.explicit.contains(&(cat.key(), name.to_lowercase()));
        let outcome = decide(exists, explicit, overwrite).unwrap_or_else(|| f(name, exists, &mut report.notes));
        match &outcome {
            Outcome::Created | Outcome::Modified => report.ok += 1,
            Outcome::Failed(_) => report.failed += 1,
            Outcome::Skipped(_) => {}
        }
        if matches!(outcome, Outcome::Created) {
            existing.add(cat, name);
        }
        // a dependency that was already there is not worth a line
        if !matches!(outcome, Outcome::Skipped("already in the drawing (dependency)")) {
            report.lines.push(format!("{} {}: {}", cat.title(), name, outcome.text()));
        }
    }
}

/// Import what `choice` names from `src`, in dependency order, as one undo step.
pub fn import_selected(ops: &mut dyn Ops, src: &Document, existing: &mut Existing, choice: &Choice) -> Report {
    let plan = make_plan(src, existing, choice);
    let mut report = Report::default();
    ops.begin_undo("Steal");
    run_cat(&mut report, existing, &plan, choice.overwrite, Cat::Linetypes, &plan.linetypes.items, |n, ex, _| {
        do_linetype(ops, src, n, ex)
    });
    run_cat(&mut report, existing, &plan, choice.overwrite, Cat::TextStyles, &plan.text_styles.items, |n, ex, _| {
        do_text_style(ops, src, n, ex)
    });
    run_cat(&mut report, existing, &plan, choice.overwrite, Cat::Layers, &plan.layers.items, |n, ex, _| {
        do_layer(ops, src, n, ex)
    });
    run_cat(&mut report, existing, &plan, choice.overwrite, Cat::DimStyles, &plan.dim_styles.items, |n, ex, notes| {
        do_dim_style(ops, src, n, ex, notes)
    });
    // blocks are never overwritten
    run_cat(&mut report, existing, &plan, false, Cat::Blocks, &plan.blocks.items, |n, _, _| do_block(ops, src, n));
    run_cat(&mut report, existing, &plan, choice.overwrite, Cat::Layouts, &plan.layouts.items, |n, ex, _| {
        do_layout(ops, src, n, ex)
    });
    ops.end_undo();
    report
}
