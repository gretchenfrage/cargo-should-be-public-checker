
use crate::{
    cli_args::CliArgs,
    error::*,
};
use std::collections::{HashMap, VecDeque, HashSet};
use rustdoc_types::*;


/// Thing which can be imported through another thing. Relative to a current crate.
#[derive(Debug, Copy, Clone)]
enum ExportRel<'a> {
    /// An item, in the current crate's `rustdoc_types::Crate::paths`.
    Item(Id),
    /// An extern crate.
    ExternCrate(&'a str),
}

/// Call `v` with all things which can be imported through this item.
fn visit_exports<'a>(item_enum: &'a ItemEnum, mut v: impl FnMut(ExportRel<'a>)) {
    match item_enum {
        &ItemEnum::Module(ref inner) => inner.items.iter().for_each(|&id| v(ExportRel::Item(id))),
        &ItemEnum::ExternCrate { ref name, .. } => v(ExportRel::ExternCrate(name)),
        &ItemEnum::Use(Use { id: Some(id), .. }) => v(ExportRel::Item(id)),
        // TODO: I am choosing not to include type aliases here. but that may be made customizable?
        _ => (),
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
struct Node {
    crate_idx: usize,
    item_id: Id,
}

#[derive(Debug)]
pub struct Graph {
    crate_lookup: HashMap<String, usize>,
    crates: Vec<CrateEntry>,

    node_queue: VecDeque<Node>,
    node_set: HashSet<Node>,

    root_node: Option<Node>,
}

#[derive(Debug)]
struct CrateEntry {
    rustdoc_crate: Crate,
    crate_module: Id,
}

impl Graph {
    pub fn build(args: &CliArgs) -> Result<Self> {
        let mut graph = Graph {
            crate_lookup: HashMap::new(),
            crates: Vec::new(),
            node_queue: VecDeque::new(),
            node_set: HashSet::new(),
            root_node: None,
        };

        let root_crate_name = args.package()?;
        let root_node = graph.add_crate(args, &root_crate_name)?;

        graph.node_queue.push_back(root_node);
        graph.node_set.insert(root_node);

        while let Some(mut node) = graph.node_queue.pop_front() {
            let rustdoc_crate = &graph.crates[node.crate_idx].rustdoc_crate;

            // paths and index should be mutually exclusive
            debug_assert!(
                rustdoc_crate.paths.get(&node.item_id).is_some()
                ^ rustdoc_crate.index.get(&node.item_id).is_some()
            );

            // canonicalize
            if let Some(item_summary) = rustdoc_crate.paths.get(&node.item_id) {
                let crate_name = &rustdoc_crate.external_crates[item_summary.crate_id].name;
            }
        }

        Ok(graph)
    }

    fn add_crate(&mut self, args: &CliArgs, crate_name: &str) -> Result<Node> {
        Ok(if let Some(&crate_idx) = self.crate_lookup.get(crate_name) {
            Node {
                crate_idx,
                item_id: self.crates[crate_idx].crate_module,
            }
        } else {
            let rustdoc_crate = args.build_rustdoc_json(crate_name)?;
            let crate_module = rustdoc_crate.index.values()
                .find(|&item|
                    matches!(&item.inner, &ItemEnum::Module(Module { is_crate: true, .. })))
                .ok_or_else(|| eyre!("Unable to find crate module for {}", crate_name))?
                .id;
            let crate_idx = self.crates.len();
            self.crates.push(CrateEntry { rustdoc_crate, crate_module });
            self.crate_lookup.insert(crate_name.into(), crate_idx);
            Node {
                crate_idx,
                item_id: crate_module,
            }
        })
    }
}
