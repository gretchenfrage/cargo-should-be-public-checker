
use crate::{
    cli_args::CliArgs,
    error::{
        Error,
        eyre,
        bail,
        OptionExt as _,
        WrapErr as _,
    },
    pretty_print::*,
};
use std::{
    collections::{
        HashMap,
        VecDeque,
    },
    fmt::{Debug, Display},
    ops::Index,
};
use rustdoc_types::*;


const STDLIBS: &'static [&'static str] = &["std", "core", "alloc", "proc_macro", "test"];


/// some rustdoc JSON id within some crate within a `GraphCache`.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub struct AbsId {
    // index of crate within GraphCache
    crate_idx: usize,
    // id of item within crates's rustdoc JSON
    item_id: Id,
}

impl AbsId {
    // elevate a rustdoc JSON id in the same crate as this one into an absolute id
    pub fn same_crate(self, item_id: Id) -> Self {
        AbsId { crate_idx: self.crate_idx, item_id }
    }
}

// "self-documenting-code" newtype wrapper for item id which is canonicalized
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub struct CanonId(pub AbsId);

// "Self-documenting-code" newtype wrapper for item id which is canonicalized and a module item
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub struct ModuleId(pub CanonId);

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
    resolve_cache: Vec<Option<ResolveCacheEntry>>,
    // maps:
    //
    // 1. rustdoc_types Id within this crate which are both canonical (their canonicalized referent
    //    is themself) and which refer to module items, to:
    // 2. path parts which can be imported directly from that module, to:
    // 3. the canoncalized referent of the importable item
    //
    // exploits rustdoc JSON Ids being distributed near zero by being a vec rather than hash map
    import_cache: Vec<Option<HashMap<String, CanonId>>>,
}

#[derive(Copy, Clone)]
enum ResolveCacheEntry {
    Id(CanonId),
    Ignore,
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

#[derive(Default)]
pub struct BfsLinker(VecDeque<Id>);

impl BfsLinker {
    pub fn link(&mut self, id: Id) {
        self.0.push_back(id);
    }

    pub fn link_all(&mut self, ids: &[Id]) {
        for &id in ids {
            self.link(id);
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

    #[allow(dead_code)] // TODO this is for debugging while building the project
    pub fn resolve2(&mut self, crate_name: &str, path: &[&str]) -> Result<CanonId, Error> {
        self.resolve_crate(crate_name)
            .and_then(|crate_id| self.resolve_path(crate_id, &path.iter().map(|&s| s.to_owned()).collect::<Vec<String>>()))
            .map_err(|e| match e {
                ResolveErr::Fail(e) => e,
                ResolveErr::Ignore => eyre!("Path is ignored")
            })
    }

    pub fn bfs(
        &mut self,
        mut link: impl FnMut(&Item, &mut BfsLinker),
        start_hash: Option<&HashMap<CanonId, String>>,
        require_public: bool,
    ) -> Result<HashMap<CanonId, String>, Error>
    {
        let mut queue: VecDeque<CanonId> = Default::default();
        let mut hash: HashMap<CanonId, String> = start_hash.cloned().unwrap_or_default();

        if let Some(start_hash) = start_hash {
            queue.extend(start_hash.keys().copied());
        } else {
            let root_crate_name = self.cli_args.root_package()?;
            let root_id = self.resolve_crate(&root_crate_name)
                .map_err(|e| match e {
                    ResolveErr::Fail(e) => e,
                    ResolveErr::Ignore => eyre!("Root crate is ignored (huh?)")
                })?;
            queue.push_back(root_id.0);
            hash.insert(root_id.0, root_crate_name.replace('-', "_"));
        }

        let mut linker: BfsLinker = Default::default();

        while let Some(id) = queue.pop_front() {
            let rustdoc_json = unsafe { self.crates[id.0.crate_idx].rustdoc_json.get() };
            let item = rustdoc_json.index.get(&id.0.item_id).unwrap();

            link(item, &mut linker);

            while let Some(iid2) = linker.0.pop_front() {
                let is_public = rustdoc_json.index.get(&iid2)
                    .is_some_and(|item|
                        item.visibility == Visibility::Public
                        || (item.visibility == Visibility::Default && match &item.inner {
                            &ItemEnum::AssocType { .. } => true, // are we being too generous here?
                            &ItemEnum::Variant(_) => true, // are we being too generous here?
                            &ItemEnum::Impl(_) => true, // are we being too generous here?
                            _ => false,
                            // TODO: the doc comment for rustdoc_types::Default is wrong here
                        })
                    );
                if require_public && !is_public {
                    // skip private item
                    continue;
                }

                const PATH_MODE: bool = false;

                match self.resolve(id.0.same_crate(iid2), true) {
                    Ok(id2) => if !hash.contains_key(&id2) {
                        // TODO: split out into function
                        let item2_name = rustdoc_json.index.get(&iid2)
                            .map(|item2| item2.name.clone().map(Some).map(Ok)
                                .unwrap_or_else(|| match &item2.inner {
                                    &ItemEnum::Impl(Impl { trait_: None, /* ref for_, TODO */ .. }) =>
                                        //Ok(Some(format!("`impl {:?}`", for_))),
                                        Ok(None),
                                    &ItemEnum::Impl(Impl { trait_: Some(ref trait_), /* ref for_, TODO */ .. }) =>
                                        //Ok(Some(format!("`impl {:?} for {:?}`", trait_, for_))),
                                        Ok(Some(format!("`<_ as {}<{:?}>>`", trait_.name, trait_.args))),
                                    &ItemEnum::ExternCrate { ref name, rename: None } =>
                                        Ok(Some(match PATH_MODE {
                                            false => name.clone(),
                                            true => format!("`pub extern crate {}`", name),
                                        })),
                                    &ItemEnum::ExternCrate { ref name, rename: Some(ref rename) } =>
                                        Ok(Some(match PATH_MODE {
                                            false => rename.clone(),
                                            true => format!("`pub extern crate {} as {}`", name, rename),
                                        })),
                                    &ItemEnum::Use(Use { ref name, is_glob: false, .. }) =>
                                        Ok(Some(match PATH_MODE {
                                            false => name.clone(),
                                            true => format!("`pub use {}`", name),
                                        })),
                                    &ItemEnum::Use(Use { ref source, is_glob: true, .. }) =>
                                        Ok(match PATH_MODE {
                                            false => None,
                                            true => Some(format!("`pub use {}::*`", source))
                                        }),
                                    _ => Err(eyre!("Unexpected lack of name for item: {:?}", item2)),
                                })
                            )
                            .unwrap_or_else(|| match PATH_MODE {
                                //false => Ok(None),
                                false => rustdoc_json.paths
                                    .get(&iid2)
                                    .ok_or_eyre("Rustdoc JSON id neither in expected index or paths")
                                    .map(|item_summary| item_summary.path.iter().rev().next().cloned()),
                                true => rustdoc_json.paths
                                    .get(&iid2)
                                    .ok_or_eyre("Rustdoc JSON id neither in expected index or paths")
                                    .map(|item_summary|
                                        Some(format!("`{}`", DisplayPath(&item_summary.path)))
                                    ),
                            })?;
                        let item2_path = item2_name
                            .map(|item2_name| format!("{}::{}", hash[&id], item2_name))
                            .unwrap_or_else(|| hash[&id].clone());
                        hash.insert(id2, item2_path);
                        if is_public {
                            // TODO: split the set here rather than requiring 2 weird phases
                            queue.push_back(id2);
                        }
                    },
                    Err(ResolveErr::Fail(e)) => {
                        let e = e.wrap_err(eyre!("Resolving child of {}", hash[&id]));
                        eprintln!("{:?}", e);
                        //return Err(e.wrap_err(eyre!("Resolving child of {}", hash[&id])))}, // TODO: better error reporting here
                    }
                    Err(ResolveErr::Ignore) => (),
                }
            }
        }

        Ok(hash)
    }

    // wrap an AbsId in a CanonId, with the possibility of debug assertion
    fn canon_id(&mut self, id: AbsId) -> CanonId {
        #[cfg(debug_assertions)]
        {
            let rustdoc_json = unsafe { self.crates[id.crate_idx].rustdoc_json.get() };
            let item = rustdoc_json.index.get(&id.item_id).expect("Canon id not internal");
            if matches!(
                &item.inner,
                &ItemEnum::ExternCrate { .. }| &ItemEnum::Use(Use { is_glob: false, .. })
            ) {
                panic!("Canon id not canon: {:?}", item);
            }
        }
        CanonId(id)
    }

    // wrap a AbsId in a ModuleId, with the possibility of debug assertion
    fn module_id(&mut self, id: AbsId) -> ModuleId {
        let id = self.canon_id(id);
        #[cfg(debug_assertions)]
        {
            let rustdoc_json = unsafe { self.crates[id.0.crate_idx].rustdoc_json.get() };
            let item = rustdoc_json.index.get(&id.0.item_id).unwrap();
            if !matches!(&item.inner, &ItemEnum::Module(_)) {
                panic!("Module id not module: {:?}", item);
            }
        }
        ModuleId(id)
    }

    // resolve the canonical id of the root of the crate with the given name
    pub fn resolve_crate(&mut self, mut crate_name: &str) -> Result<ModuleId, ResolveErr> {
        if crate_name == "webpki" {
            crate_name = "rustls_webpki"; // TODO
        }

        if STDLIBS.contains(&crate_name) {
            return Err(ResolveErr::Ignore);
        }

        if let Some(&crate_idx) = self.crate_lookup.get(crate_name) {
            // cached
            return Ok(self.module_id(AbsId {
                crate_idx,
                item_id: self.crates[crate_idx].root_module,
            }));
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
        Ok(self.module_id(AbsId { crate_idx, item_id: root_module }))
    }

    // resolve the canonical referent of the given id (with caching)
    pub fn resolve(&mut self, id: AbsId, filter_public: bool) -> Result<CanonId, ResolveErr> {
        if let Some(&entry) = self.crates[id.crate_idx].resolve_cache
            .get(id.item_id.0 as usize)
            .and_then(|opt| opt.as_ref())
        {
            // cached
            return match entry {
                ResolveCacheEntry::Id(id) => Ok(id),
                ResolveCacheEntry::Ignore => Err(ResolveErr::Ignore),
            };
        } else {
            // must cache
            let result = self.resolve_inner(id, filter_public);
            let cache = &mut self.crates[id.crate_idx].resolve_cache;
            while cache.len() <= id.item_id.0 as usize {
                cache.push(None);
            }
            let cache_slot = &mut cache[id.item_id.0 as usize];
            match result {
                Ok(id) => {
                    *cache_slot = Some(ResolveCacheEntry::Id(id));
                    Ok(id)
                }
                Err(ResolveErr::Ignore) => {
                    *cache_slot = Some(ResolveCacheEntry::Ignore);
                    Err(ResolveErr::Ignore)
                }
                Err(ResolveErr::Fail(e)) => Err(ResolveErr::Fail(e)),
            }
        }
    }

    // resolve the canonical referent of the given id (no caching)
    fn resolve_inner(&mut self, id: AbsId, filter_public: bool) -> Result<CanonId, ResolveErr> {
        if let Some(&entry) = self.crates[id.crate_idx].resolve_cache
            .get(id.item_id.0 as usize)
            .and_then(|opt| opt.as_ref())
        {
            // cached
            return match entry {
                ResolveCacheEntry::Id(id) => Ok(id),
                ResolveCacheEntry::Ignore => Err(ResolveErr::Ignore),
            };
        }

        let rustdoc_json = unsafe { self.crates[id.crate_idx].rustdoc_json.get() };

        Ok(if let Some(item) = rustdoc_json.index.get(&id.item_id) {
            if filter_public && item.visibility != Visibility::Public {
                // ignore private item
                //return Err(ResolveErr::Ignore);
                // TODO
            }

            // id internal to its crate, attempt to make progress via it being a reexport
            match &item.inner {
                &ItemEnum::ExternCrate { ref name, .. } =>
                    self.resolve_crate(name).wrap_err("Resolving `pub extern crate` item")?.0,
                &ItemEnum::Use(Use { ref source, ref name, id: Some(iid2), is_glob: false }) =>
                    // in resolving the referent, filter_public becomes false, since a `pub use`
                    // re-export _can_ make a private item publically accessible.
                    self.resolve(id.same_crate(iid2), false).wrap_err_with(|| eyre!(
                        "Resolving referent of `pub use {} as {}`", source, name
                    ))?,
                // note: intentionally not including type/trait aliases, but that's a debatable
                //       design decision

                // base case, already canonical
                _ => self.canon_id(id),
            }
        } else {
            // id external to its crate, make progress by jumping to an internal id
            let item_summary = rustdoc_json.paths.get(&id.item_id)
                .ok_or_eyre("Rustdoc JSON id neither in expected index or paths")?;
            let crate_name = &rustdoc_json.external_crates.get(&item_summary.crate_id)
                .ok_or_eyre("Rustdoc JSON external crates key missing")?
                .name;
            //let crate_name = item_summary.path.get(0)
            //    .ok_or_eyre("Rustdoc JSON ItemSummary with empty path")?;
            let crate_id = self.resolve_crate(crate_name).wrap_err_with(|| eyre!(
                "Resolving ItemSummary crate {:?} for {:?}", crate_name, item_summary.path
            ))?;
            self.resolve_path(crate_id, &item_summary.path[1..])
                .wrap_err_with(|| eyre!(
                    "Resolving item in other crate: {}", DisplayPath(&item_summary.path)
                ))?
        })
    }

    // given the canonical id of a module item, resolve the canonical referent of importing it
    // followed by the given path
    fn resolve_path(&mut self, id: ModuleId, path: &[String]) -> Result<CanonId, ResolveErr> {
        let mut id = id.0;
        for path_part in path {
            if path_part == "__private" {
                return Err(ResolveErr::Ignore); // TODO handle this better
            }

            // build namespace
            let namespace = self.module_namespace(id)
                .wrap_err_with(|| eyre!("Resolving path part {:?} of {:?}", path_part, path))?;

            // look up item in namespace
            id = *namespace.get(path_part)
                .ok_or_else(|| eyre!(
                    "Unable to find importable item: {} (importable names={:?})", path_part, &namespace.keys()
                ))?;
        }
        Ok(id)
    }

    // given the canonical id of an item, validate that it's a module item, and build mapping of
    // the names and corresponding canonical referents of all items which can be imported directly
    // through it (cache it, and return a reference to the cache)
    fn module_namespace(&mut self, id: CanonId) -> Result<&HashMap<String, CanonId>, Error> {
        let cached = self.crates[id.0.crate_idx].import_cache
            .get(id.0.item_id.0 as usize)
            .and_then(|opt| opt.as_ref());
        if cached.is_none() {
            let namespace = self.module_namespace_inner(id)?;
            let cache = &mut self.crates[id.0.crate_idx].import_cache;
            while cache.len() <= id.0.item_id.0 as usize {
                cache.push(None);
            }
            cache[id.0.item_id.0 as usize] = Some(namespace);
        }
        Ok(self.crates[id.0.crate_idx].import_cache
            .get(id.0.item_id.0 as usize)
            .and_then(|opt| opt.as_ref())
            .unwrap())
    }

    // like module_namespace but without no caching
    fn module_namespace_inner(&mut self, id: CanonId) -> Result<HashMap<String, CanonId>, Error> {
        // ensure the module_id refers to a module item
        let rustdoc_json = unsafe { self.crates[id.0.crate_idx].rustdoc_json.get() };
        let item = &rustdoc_json.index.get(&id.0.item_id).unwrap();
        let &ItemEnum::Module(ref module) = &item.inner
            else { bail!("Cannot import from non-module") };

        let mut namespace = HashMap::new();

        // iterate through its children
        for &child_iid in &module.items {
            // canonicalize the child
            let child_id = id.0.same_crate(child_iid);
            let child_id = match self.resolve(child_id, false) {
                Ok(child_id) => child_id,
                Err(ResolveErr::Fail(e)) => return Err(e),
                Err(ResolveErr::Ignore) => continue,
            };
            let child_rustdoc_json = unsafe { self.crates[child_id.0.crate_idx].rustdoc_json.get() };
            let child_item = child_rustdoc_json.index.get(&child_id.0.item_id).unwrap();

            if let Some(child_name) = child_item.name.as_ref() {
                // child is importable
                namespace.insert(child_name.clone(), child_id);
            } else if let &ItemEnum::Use(Use {
                id: Some(glob_imported_iid),
                is_glob: true,
                ..
            }) = &child_item.inner {
                // child is a glob import of another module, thus the other module's namespaces
                // gets unioned into this one

                // canonicalize the glob-imported module id
                let glob_imported_id = child_id.0.same_crate(glob_imported_iid);
                let glob_imported_id = match self.resolve(glob_imported_id, false) {
                    Ok(glob_imported_id) => glob_imported_id,
                    Err(ResolveErr::Fail(e)) => return Err(e),
                    Err(ResolveErr::Ignore) => continue,
                };

                // flatten the glob-imported module's namespace into our own
                let glob_imported_namespace = self.module_namespace_inner(glob_imported_id)
                    .wrap_err("Unioning in namespace from glob import")?;
                namespace.extend(glob_imported_namespace);
            }
        }
        Ok(namespace)
    }
}

impl<'a> Index<CanonId> for GraphCache<'a> {
    type Output = Item;

    fn index(&self, id: CanonId) -> &Item {
        let rustdoc_json = unsafe { self.crates[id.0.crate_idx].rustdoc_json.get() };
        rustdoc_json.index.get(&id.0.item_id).unwrap()
    }
}

pub enum ResolveErr {
    Fail(Error),
    Ignore,
}

impl From<Error> for ResolveErr {
    fn from(e: Error) -> Self {
        ResolveErr::Fail(e)
    }
}

impl ResolveErr {
    fn wrap_err<D: Display + Send + Sync + 'static>(self, msg: D) -> Self {
        match self {
            ResolveErr::Fail(e) => ResolveErr::Fail(e.wrap_err(msg)),
            ResolveErr::Ignore => ResolveErr::Ignore,
        }
    }
}

trait WrapErr2<T> {
    fn wrap_err<D: Display + Send + Sync + 'static>(self, msg: D) -> Result<T, ResolveErr>;

    fn wrap_err_with<D, F>(self, f: F) -> Result<T, ResolveErr>
    where
        D: Display + Send + Sync + 'static,
        F: FnOnce() -> D;
}

impl<T> WrapErr2<T> for Result<T, ResolveErr> {
    fn wrap_err<D: Display + Send + Sync + 'static>(self, msg: D) -> Result<T, ResolveErr> {
        self.map_err(|e| e.wrap_err(msg))
    }

    fn wrap_err_with<D, F>(self, f: F) -> Result<T, ResolveErr>
    where
        D: Display + Send + Sync + 'static,
        F: FnOnce() -> D
    {
        self.map_err(|e| e.wrap_err(f()))
    }
}
