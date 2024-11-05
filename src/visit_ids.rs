
use crate::{
    cli_args::CliArgs,
    error::{
        Error,
        eyre,
        bail,
        OptionExt as _,
    },
};
use std::{
    collections::{
        HashMap,
        VecDeque,
        HashSet,
    },
    fmt::{Debug, Display},
};
use rustdoc_types::*;


const STDLIBS: &'static [&'static str] = &["std", "core", "alloc", "proc_macro", "test"];


/// Some rustdoc JSON id within some crate within a `GraphCache`.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub struct AbsId {
    // index of crate within GraphCache
    crate_idx: usize,
    // id of item within crates's rustdoc JSON
    item_id: Id,
}

impl AbsId {
    // elevant a rustdoc JSON id in the same crate as this one into an absolute id
    fn same_crate(self, item_id: Id) -> Self {
        AbsId { crate_idx: self.crate_idx, item_id }
    }
}

/// Lazy cache for use in traversing graphs of rustdoc JSON items across multiple crates.
pub struct GraphCache<'a> {
    pub cli_args: &'a CliArgs,
    // maps crate name -> crate index
    crate_lookup: HashMap<String, usize>,
    // maps crate index -> data about the crate
    crates: Vec<CrateEntry>,
}

struct CrateEntry {
    // crate's rustdoc JSON output
    rustdoc_json: CrateRustdocJsonCell,
    // Id within this rustdoc JSON index of the module item representing the crate root
    root_module: Id,
    // maps rustdoc_types Id within this crate -> its canonicalized referent
    //
    // exploits rustdoc JSON Ids being distributed near zero by being a vec rather than hash map
    resolve_cache: Vec<Option<AbsId>>,
    // maps:
    //
    // 1. rustdoc_types Id within this crate which are both canonical (their canonicalized referent
    //    is themself) and which refer to module items, to:
    // 2. path parts which can be imported directly from that module, to:
    // 3. the canoncalized referent of the importable item
    //
    // exploits rustdoc JSON Ids being distributed near zero by being a vec rather than hash map
    import_cache: Vec<Option<HashMap<String, AbsId>>>,
}

struct CrateRustdocJsonCell(*mut Crate);

impl CrateRustdocJsonCell {
    // get the rustdoc JSON output, just making up whatever lifetime for it.
    //
    // blanket note on safety of usages:
    //
    // - this is owned by GraphCache in an add-only cache (so not dropped until GraphCache dropped)
    // - this is not mutated until dropped (so exclusive borrows not a concern)
    // - this is accessed through a non-unique pointer
    //   (so exclusive borrows to other parts of CrateEntry are ok)
    // - this is heap-allocated (so its memory address wont change)
    //
    // thus this is fine to use for lifetimes that don't extend past the lifetime of the GraphCache
    unsafe fn get<'a, 'b>(&'a self) -> &'b Crate {
        &*(self.0 as *const Crate)
    }
}

impl From<Crate> for CrateRustdocJsonCell {
    fn from(rustdoc_json: Crate) -> Self {
        CrateRustdocJsonCell(Box::into_raw(Box::new(rustdoc_json)))
    }
}

impl Drop for CrateRustdocJsonCell {
    fn drop(&mut self) {
        unsafe {
            drop(Box::from_raw(self.0));
        }
    }
}

impl<'a> GraphCache<'a> {
    pub fn new(cli_args: &'a CliArgs) -> Self {
        GraphCache {
            cli_args,
            crate_lookup: Default::default(),
            crates: Default::default(),
        }
    }

    /// Get the item ID representing (the root module of) the crate of the given name, invoking
    /// rustdoc JSON if necessary.
    fn resolve_crate(&mut self, crate_name: &str) -> Result<AbsId, ResolveErr> {
        Ok(if let Some(&crate_idx) = self.crate_lookup.get(crate_name) {
            // cached
            AbsId {
                crate_idx,
                item_id: self.crates[crate_idx].root_module,
            }
        } else {
            if STDLIBS.contains(&crate_name) {
                return Err(ResolveErr::Ignore);
            }

            let crate_idx = self.crates.len();
            let rustdoc_json = self.cli_args.build_rustdoc_json(crate_name)?;
            let root_module = rustdoc_json.index.values()
                .find(|&item|
                    matches!(&item.inner, &ItemEnum::Module(Module { is_crate: true, .. })))
                .ok_or_else(|| eyre!("No root module in rustdoc JSON of {:?} crate", crate_name))?
                .id;
            self.crates.push(CrateEntry {
                rustdoc_json: rustdoc_json.into(),
                root_module,
                resolve_cache: Default::default(),
                import_cache: Default::default(),
            });
            self.crate_lookup.insert(crate_name.to_owned(), crate_idx);
            AbsId { crate_idx, item_id: root_module }
        })
    }

    pub fn resolve_path(&mut self, path: &[&str]) -> Result<AbsId, Error> {
        self.resolve(None, path, 0).map_err(|e| match e {
            ResolveErr::Fail(e) => e,
            ResolveErr::Ignore => eyre!("Path is ignored")
        })
    }

    // resolve the canonicalized referent of:
    //
    // - if id is some and path is empty, id
    // - if id is none and path is non-empty, the result of importing path
    // - if id is some and path is non-empty, the result of importing some path with resolves to
    //   the same canonicalized referent of id, adjoined with the given path
    fn resolve<P: AsRef<str> + Debug>(
        &mut self,
        id: Option<AbsId>,
        path: &[P],
        dbg_indent: u32,
    ) -> Result<AbsId, ResolveErr> {
        // TODO: caching
        for _ in 0..dbg_indent {
            eprint!("  ");
        }
        eprintln!("resolve({:?}, {:?})", id, path);

        // we must ensure that each recursive iteration makes forward progress
        Ok(if let Some(id) = id {
            // resolve starting with an id
            let rustdoc_json = unsafe { self.crates[id.crate_idx].rustdoc_json.get() };

            // attempt to make progress on the id, or set id2 to None if id is already canonical
            let id2: Option<AbsId> = if let Some(item) = rustdoc_json.index.get(&id.item_id) {
                // id internal to its crate, attempt to make progress via it being a reexport
                match &item.inner {
                    &ItemEnum::ExternCrate { ref name, .. } =>
                        Some(self.resolve(None, &[name], dbg_indent + 1)?),
                    &ItemEnum::Use(Use { id: Some(iid2), is_glob: false, .. }) =>
                        Some(id.same_crate(iid2)),
                    // TODO: type alias is, I think, no?
                    // TODO: trait alias is, I think, no, but also, unimplemented due to unstable
                    _ => None,
                }
            } else {
                // id external to its crate, make progress by jumping to an internal id
                let item_summary = rustdoc_json.paths.get(&id.item_id)
                    .ok_or_eyre("Id neither in expected rustdoc JSON index or paths")?;
                Some(self.resolve(None, &item_summary.path, dbg_indent + 1)?)
            };

            // if we cannot make forward progress on the id, then it must be canonicalized
            if let Some(id2) = id2 {
                // we did make forward progress on id, iterate
                self.resolve(Some(id2), path, dbg_indent + 1)?
            } else if path.is_empty() {
                // base case: id is canonical and path is empty
                id
            } else {
                // id is canonical, so make forward progress on path instead
                // TODO: cache this too
                let mut namespace = HashMap::new();
                self.build_namespace(id, &mut namespace, dbg_indent + 1)?;
                let &id2 = namespace.get(path[0].as_ref())
                    .ok_or_else(|| eyre!("Unable to import path part {}", &path[0].as_ref()))?;

                // iterate
                self.resolve(Some(id2), path, dbg_indent + 1)?
            }
        } else {
            // resolve starting with a crate name, making progress on the path
            if path.is_empty() {
                return Err(eyre!("Attempted to resolve empty path").into());
            }
            let crate_id = self.resolve_crate(&path[0].as_ref())?;
            self.resolve(Some(crate_id), &path[1..], dbg_indent + 1)?
        })
    }

    // given a canonicalized id of a module item, add to namespace the names and canonicalized ids
    // of all items which can be directly imported through that module.
    fn build_namespace(
        &mut self,
        module_id: AbsId,
        namespace: &mut HashMap<String, AbsId>,
        dbg_indent: u32,
    ) -> Result<(), Error> {
        for _ in 0..dbg_indent {
            eprint!("  ");
        }
        eprintln!("build_namespace({:?})", module_id);

        let rustdoc_json = unsafe { self.crates[module_id.crate_idx].rustdoc_json.get() };
        let module_item = &rustdoc_json.index.get(&module_id.item_id).expect("unreachable");
        let &ItemEnum::Module(ref module) = &module_item.inner
            else { bail!("Cannot import from non-module") };

        for &child_iid in &module.items {
            let mut child_id = module_id.same_crate(child_iid);
            child_id = match self.resolve::<&str>(Some(child_id), &[], dbg_indent + 1) {
                Ok(child_id) => child_id,
                Err(ResolveErr::Fail(e)) => return Err(e),
                Err(ResolveErr::Ignore) => continue,
            };
            let child_rustdoc_json =
                unsafe { self.crates[child_id.crate_idx].rustdoc_json.get() };
            let child_item = child_rustdoc_json.index.get(&child_id.item_id).expect("unreachable");

            if let Some(child_name) = child_item.name.as_ref() {
                // module contains an importable item
                namespace.insert(child_name.clone(), child_id);
            } else if let &ItemEnum::Use(Use {
                id: Some(glob_imported_iid),
                is_glob: true,
                ..
            }) = &child_item.inner {
                // namespace contains a glob import, and thus everything in the glob-imported
                // module can be imported from this module _directly_

                // canonicalize the glob-imported id
                let mut glob_imported_id = child_id.same_crate(glob_imported_iid);
                glob_imported_id = match self.resolve::<&str>(Some(glob_imported_id), &[], dbg_indent + 1) {
                    Ok(glob_imported_id) => glob_imported_id,
                    Err(ResolveErr::Fail(e)) => return Err(e),
                    Err(ResolveErr::Ignore) => continue,
                };

                // flatten the glob-imported module's namespace into our own
                self.build_namespace(glob_imported_id, namespace, dbg_indent + 1)?;
            }
        }
        Ok(())
    }
}

// TODO: we may have to respect is_stripped in publicness detection

enum ResolveErr {
    Fail(Error),
    Ignore,
}

impl From<Error> for ResolveErr {
    fn from(e: Error) -> Self {
        ResolveErr::Fail(e)
    }
}
