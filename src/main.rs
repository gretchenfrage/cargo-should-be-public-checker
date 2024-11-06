
use crate::{
    cli_args::CliArgs,
    item_graph::{
        GraphCache,
        BfsLinker,
    },
    error::*,
};
use clap::Parser;
use rustdoc_types::*;

pub mod error {
    pub use color_eyre::eyre::*;
}

mod cli_args;
mod build_rustdoc_json;
mod cargo_metadata;
mod item_graph;

fn link_importable<'i>(item: &'i Item, bfs: &mut BfsLinker<'i>) {
    if item.visibility != Visibility::Public {
        return; // not quite
    }

    match &item.inner {
        &ItemEnum::Module(ref module) => {
            for &id in &module.items {
                bfs.link(id);
            }
        }
        &ItemEnum::Use(Use { is_glob: true, id: Some(id), .. }) => bfs.link(id),
        _ => (),
    }
}


fn main() -> Result<()> {
    let args = CliArgs::parse();
    color_eyre::install()?;
    let mut graph = GraphCache::new(&args);
    //dbg!(graph.resolve2("quinn", &["StreamId"])?);
    let importable = graph.bfs(link_importable)?;
    println!("importable:");
    for &id in &importable {
        if graph[id].visibility != Visibility::Public {
            continue; // TODO
        }
        println!("- {:?}", graph[id].name);
    }

    Ok(())
}
