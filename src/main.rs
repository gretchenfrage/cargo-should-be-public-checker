
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
mod pretty_print;

// bfs linker that finds all items which can be imported from the root crate
fn link_importable(item: &Item, bfs: &mut BfsLinker) {
    match &item.inner {
        &ItemEnum::Module(ref module) => bfs.link_all(&module.items),
        &ItemEnum::Use(Use { is_glob: true, id: Some(id), .. }) => bfs.link(id),
        _ => (),
    }
}

// bfs linker that finds all items which are a part of the root crate's API surface
fn link_visible(item: &Item, bfs: &mut BfsLinker) {
    match &item.inner {
        &ItemEnum::Module(_) => (), // all contents already marked as importable
        &ItemEnum::ExternCrate { .. } => unreachable!("not canonical"),
        &ItemEnum::Use(Use { is_glob: true, .. }) => (), // all contents already marked as importable
        &ItemEnum::Use(Use { is_glob: false, .. }) => unreachable!("not canonical"),
        &ItemEnum::Union(ref inner) => {
            link_visible_generics(&inner.generics, bfs);
            bfs.link_all(&inner.fields);
            bfs.link_all(&inner.impls);
        }
        &ItemEnum::Struct(ref inner) => {
            match &inner.kind {
                &StructKind::Unit => (),
                &StructKind::Tuple(ref fields) =>
                    for field in fields {
                        if let &Some(field) = field {
                            bfs.link(field);
                        }
                    },
                &StructKind::Plain { ref fields, .. } => bfs.link_all(fields),
            }
            link_visible_generics(&inner.generics, bfs);
            bfs.link_all(&inner.impls);
        }
        &ItemEnum::StructField(ref type_) => link_visible_type(type_, bfs),
        &ItemEnum::Enum(ref inner) => {
            link_visible_generics(&inner.generics, bfs);
            bfs.link_all(&inner.variants);
            bfs.link_all(&inner.impls);
        }
        &ItemEnum::Variant(ref inner) => {
            match &inner.kind {
                &VariantKind::Plain => (),
                &VariantKind::Tuple(ref fields) =>
                    for field in fields {
                        if let &Some(field) = field {
                            bfs.link(field);
                        }
                    },
                &VariantKind::Struct { ref fields, .. } => bfs.link_all(fields),
            }
        }
        &ItemEnum::Function(ref inner) => {
            link_visible_function_signature(&inner.sig, bfs);
            link_visible_generics(&inner.generics, bfs);
        }
        &ItemEnum::Trait(ref inner) => {
            bfs.link_all(&inner.items);
            link_visible_generics(&inner.generics, bfs);
            for bound in &inner.bounds {
                link_visible_generic_bound(bound, bfs);
            }
            // TODO: inner.implementations exists, but we need to have a way of knowing whether a
            //       trait impl is effectively public
        }
        &ItemEnum::TraitAlias(_) => unimplemented!(),
        &ItemEnum::Impl(ref inner) => {
            // TODO: we need to have a way of knowing whether a trait impl is effectively public
            link_visible_generics(&inner.generics, bfs);
            // TODO: impl.trait_ exists
            // TODO: impl.for_ exists
            bfs.link_all(&inner.items);
            // TODO: blanket_impl exists, and is lacking documentation
        }
        &ItemEnum::TypeAlias(ref inner) => {
            link_visible_type(&inner.type_, bfs);
            link_visible_generics(&inner.generics, bfs);
        }
        &ItemEnum::Constant { ref type_, .. } => link_visible_type(type_, bfs),
        &ItemEnum::Static(ref inner) => link_visible_type(&inner.type_, bfs),
        &ItemEnum::ExternType => unimplemented!(),
        &ItemEnum::Macro(_) => (),
        &ItemEnum::ProcMacro(_) => (),
        &ItemEnum::Primitive(_) => (),
        &ItemEnum::AssocConst { ref type_, .. } => link_visible_type(type_, bfs),
        &ItemEnum::AssocType { ref generics, ref bounds, ref type_ } => {
            link_visible_generics(generics, bfs);
            for bound in bounds {
                link_visible_generic_bound(bound, bfs);
            }
            if let Some(type_) = type_ {
                link_visible_type(type_, bfs);
            }
        }
    }
}

fn link_visible_generics(generics: &Generics, bfs: &mut BfsLinker) {
    for param in &generics.params {
        link_visible_generic_param(param, bfs);
    }
    for where_predicate in &generics.where_predicates {
        match where_predicate {
            &WherePredicate::BoundPredicate { ref type_, ref bounds, ref generic_params } => {
                link_visible_type(type_, bfs);
                for bound in bounds {
                    link_visible_generic_bound(bound, bfs);
                }
                for param in generic_params {
                    link_visible_generic_param(param, bfs);
                }
            }
            &WherePredicate::LifetimePredicate { .. } => (),
            &WherePredicate::EqPredicate { ref lhs, ref rhs } => {
                link_visible_type(lhs, bfs);
                link_visible_term(rhs, bfs);
            }
        }
    }
}

fn link_visible_term(term: &Term, bfs: &mut BfsLinker) {
    if let &Term::Type(ref type_) = term {
        link_visible_type(type_, bfs);
    }
}

fn link_visible_generic_param(param: &GenericParamDef, bfs: &mut BfsLinker) {
    if let &GenericParamDefKind::Type { ref bounds, ref default, .. } = &param.kind {
        for bound in bounds {
            link_visible_generic_bound(bound, bfs);
        }
        if let &Some(ref default) = default {
            link_visible_type(default, bfs);
        }
    }
}

fn link_visible_generic_bound(bound: &GenericBound, bfs: &mut BfsLinker) {
    if let &GenericBound::TraitBound { ref trait_, ref generic_params, .. } = bound {
        link_visible_path(&trait_, bfs);
        for param2 in generic_params {
            link_visible_generic_param(param2, bfs);
        }
    }
    // TODO GenericBound::Use, once stable
}

fn link_visible_path(path: &Path, bfs: &mut BfsLinker) {
    bfs.link(path.id);
    if let &Some(ref args) = &path.args {
        link_visible_generic_args(&**args, bfs);
    }
}

fn link_visible_generic_args(args: &GenericArgs, bfs: &mut BfsLinker) {
    match args {
        &GenericArgs::AngleBracketed { ref args, ref constraints } => {
            for arg in args {
                match arg {
                    &GenericArg::Lifetime(_) => (),
                    &GenericArg::Type(ref type_) => link_visible_type(type_, bfs),
                    &GenericArg::Const(_) => (),
                    &GenericArg::Infer => (),
                }
            }
            for constraint in constraints {
                link_visible_generic_args(&constraint.args, bfs);
                match &constraint.binding {
                    &AssocItemConstraintKind::Equality(ref term) => link_visible_term(term, bfs),
                    &AssocItemConstraintKind::Constraint(ref bounds) =>
                        for bound in bounds {
                            link_visible_generic_bound(bound, bfs);
                        },
                }
            }
        }
        &GenericArgs::Parenthesized { ref inputs, ref output } => {
            for input in inputs {
                link_visible_type(input, bfs);
            }
            if let Some(output) = output {
                link_visible_type(output, bfs);
            }
        }
    }
}

fn link_visible_type(type_: &Type, bfs: &mut BfsLinker) {
    match type_ {
        &Type::ResolvedPath(ref path) => link_visible_path(path, bfs),
        &Type::DynTrait(ref dyn_trait) =>
            for trait_ in &dyn_trait.traits {
                link_visible_path(&trait_.trait_, bfs);
                for param in &trait_.generic_params {
                    link_visible_generic_param(param, bfs);
                }
            },
        &Type::Generic(_) => (),
        &Type::Primitive(_) => (),
        &Type::FunctionPointer(ref function_pointer) => {
            link_visible_function_signature(&function_pointer.sig, bfs);
            for param in &function_pointer.generic_params {
                link_visible_generic_param(param, bfs);
            }
        }
        &Type::Tuple(ref types) => {
            for type_ in types {
                link_visible_type(type_, bfs);
            }
        }
        &Type::Slice(ref type_) => link_visible_type(&**type_, bfs),
        &Type::Array { ref type_, .. } => link_visible_type(&**type_, bfs),
        &Type::Pat { .. } => unimplemented!(),
        &Type::ImplTrait(ref bounds) =>
            for bound in bounds {
                link_visible_generic_bound(bound, bfs)
            },
        &Type::Infer => (),
        &Type::RawPointer { ref type_, .. } => link_visible_type(&**type_, bfs),
        &Type::BorrowedRef { ref type_, .. } => link_visible_type(&**type_, bfs),
        &Type::QualifiedPath { ref args, ref self_type, ref trait_, .. } => {
            link_visible_generic_args(&**args, bfs);
            link_visible_type(&**self_type, bfs);
            if let &Some(ref trait_) = trait_ {
                link_visible_path(trait_, bfs);
            }
        }
    }
}

fn link_visible_function_signature(sig: &FunctionSignature, bfs: &mut BfsLinker) {
    for &(_, ref type_) in &sig.inputs {
        link_visible_type(type_, bfs);
    }
    if let &Some(ref type_) = &sig.output {
        link_visible_type(type_, bfs);
    }
}

fn main() -> Result<()> {
    let args = CliArgs::parse();
    color_eyre::install()?;
    let mut graph = GraphCache::new(&args);
    //dbg!(graph.resolve2("quinn", &["StreamId"])?);
    let importable = graph.bfs(link_importable, None, true)?;
    /*println!("importable:");
    let mut paths = importable.values().cloned().collect::<Vec<_>>();
    paths.sort();
    for path in &paths {
        println!("- {}", path);
    }*/
    let visible = graph.bfs(link_visible, Some(&importable), false)?;
    println!("visible but not importable:");
    let mut paths = visible.iter()
        .filter(|&(&id, _)| !importable.contains_key(&id))
        .filter(|&(&id, _)| match &graph[id].inner {
            &ItemEnum::Union(_) => true,
            &ItemEnum::Struct(_) => true,
            &ItemEnum::Enum(_) => true,
            &ItemEnum::Trait(_) => true,
            &ItemEnum::TraitAlias(_) => unimplemented!(),
            &ItemEnum::TypeAlias(_) => true,
            &ItemEnum::ExternType => unimplemented!(),
            _ => false,
        })
        .map(|(_, path)| path.clone())
        .collect::<Vec<_>>();
    paths.sort();
    for path in &paths {
        println!("- {}", path);
    }
    /*/
    let stream_id_id = graph.resolve2("quinn", &["StreamId"])?;
    dbg!(&graph[stream_id_id]);
    let &ItemEnum::Struct(ref inner) = &graph[stream_id_id].inner else { panic!() };
    for &impl_iid in &inner.impls.clone() {
        let impl_id = graph.resolve(stream_id_id.0.same_crate(impl_iid), true).ok().unwrap();
        let &ItemEnum::Impl(ref inner) = &graph[impl_id].inner else { panic!() };
        if inner.trait_.is_some() { continue; }
        dbg!(&graph[impl_id]);
        for &item_iid in &inner.items.clone() {
            let item_id = graph.resolve(impl_id.0.same_crate(item_iid), true).ok().unwrap();
            if graph[item_id].name.as_ref().map(|s| s.as_str()) != Some("dir") { continue; }
            dbg!(&graph[item_id]);
            let &ItemEnum::Function(ref inner) = &graph[item_id].inner else { panic!() };
            let &Type::ResolvedPath(ref inner) = inner.sig.output.as_ref().unwrap() else { panic!() };
            let dir_id = graph.resolve(item_id.0.same_crate(inner.id), true).ok().unwrap();
            dbg!(&graph[dir_id]);
        }
    }
    */
    Ok(())
}
