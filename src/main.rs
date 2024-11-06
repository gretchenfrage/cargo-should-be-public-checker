
use crate::{
    cli_args::CliArgs,
    error::*,
};
use clap::Parser;

pub mod error {
    pub use color_eyre::eyre::*;
}

mod cli_args;
mod build_rustdoc_json;
mod cargo_metadata;
mod item_graph;




fn main() -> Result<()> {
    let args = CliArgs::parse();
    color_eyre::install()?;
    let mut graph = item_graph::GraphCache::new(&args);
    dbg!(graph.resolve2("quinn", &["StreamId"])?);
    //visit_ids::Graph::build(&args)?;
    //use rustdoc_types::*;
    //let c = args.build_rustdoc_json()?;
    //println!("{:#?}", c);
    /*for i in c.index.values().filter(|&item| matches!(&item.inner, &ItemEnum::Use(_))) {
        //println!("{:#?}", i);
    }
    println!("{:#?}", &c.index[&Id(577)]);*/
    /*let found = c.index.values()
        .find(|&item| match &item.inner {
            &ItemEnum::Use(ref inner) => true || inner.source == "quinn_proto::StreamId",
            _ => false,
        });
    dbg!(found);*/

    Ok(())
}
