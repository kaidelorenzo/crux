mod filter;
mod formatter;
pub mod generate;
mod indexed;
mod item;
mod node;
mod serde;
mod serde_generate;

use std::{
    collections::{BTreeMap, HashMap},
    fs::{self, File},
    io::Read,
    path::PathBuf,
    process::Command,
};

use anyhow::{anyhow, bail, Context, Result};
use guppy::{graph::PackageGraph, MetadataCommand};
use log::debug;
use rustdoc_types::Crate;

use crate::args::CodegenArgs;
use filter::Filter;
use formatter::Formatter;
use node::ItemNode;
use serde_generate::format::ContainerFormat;

pub type Registry = BTreeMap<String, ContainerFormat>;

pub fn codegen(args: &CodegenArgs) -> Result<()> {
    let mut cmd = MetadataCommand::new();
    let package_graph = PackageGraph::from_command(&mut cmd)?;

    let manifest_paths: BTreeMap<&str, &str> = package_graph
        .packages()
        .map(|package| (package.name(), package.manifest_path().as_str()))
        .collect();

    let Ok(lib) = package_graph.workspace().member_by_path(&args.lib) else {
        bail!("Could not find workspace package with path {}", args.lib)
    };

    let lib_name = lib.name();

    let registry = run(lib_name, |name| load_crate(name, &manifest_paths))?;

    // switch from vendored types to `serde-reflection` types
    let registry: serde_reflection::Registry =
        serde_json::from_slice(&serde_json::to_vec(&registry)?)?;

    fs::create_dir_all(&args.output)?;

    generate::java(&registry, &args.java_package, args.output.join("java"))
        .context("Generating types for Java")?;

    generate::swift(&registry, &args.swift_package, args.output.join("swift"))
        .context("Generating tyeps for Swift")?;

    generate::typescript(
        &registry,
        &args.typescript_package,
        &lib.version().to_string(),
        args.output.join("typescript"),
    )
    .context("Generating types for TypeScript")?;
    Ok(())
}

fn run<F>(crate_name: &str, load: F) -> Result<Registry>
where
    F: Fn(&str) -> Result<Crate>,
{
    let mut previous: HashMap<String, Crate> = HashMap::new();

    let shared_lib = load(crate_name)?;

    let mut filter = Filter::default();
    filter.process(crate_name, &shared_lib)?;

    previous.insert(crate_name.to_string(), shared_lib);

    let mut next: Vec<String> = filter.get_crates();

    while let Some(crate_name) = next.pop() {
        if previous.contains_key(&crate_name) {
            continue;
        }
        let crate_ = load(&crate_name)?;

        filter.process(&crate_name, &crate_)?;

        next = filter.get_crates();
        previous.insert(crate_name, crate_);
    }

    Ok(format(filter.edge))
}

fn format(edges: Vec<(ItemNode, ItemNode)>) -> Registry {
    let mut formatter = Formatter::default();
    formatter.edge = edges;
    formatter.run();
    debug!("{}", formatter.scc_times_summary());

    formatter.container.into_iter().collect()
}

fn load_crate(name: &str, manifest_paths: &BTreeMap<&str, &str>) -> Result<Crate> {
    // TODO: ensure that the user has installed the core rustdoc JSON files
    // e.g. `rustup component add --toolchain nightly rust-docs-json`

    let json_path = if let "core" | "alloc" | "std" = name {
        rustdoc_json_path()?.join(format!("{name}.json"))
    } else {
        let manifest_path = manifest_paths
            .get(name)
            .ok_or_else(|| anyhow!("unknown crate {}", name))?;
        rustdoc_json::Builder::default()
            .toolchain("nightly")
            .document_private_items(true)
            .manifest_path(manifest_path)
            .build()?
    };
    debug!("from {}", json_path.to_string_lossy());

    let buf = &mut Vec::new();
    File::open(json_path)?.read_to_end(buf)?;
    let crate_ = serde_json::from_slice(buf)?;

    Ok(crate_)
}

fn rustdoc_json_path() -> Result<PathBuf> {
    let output = Command::new("rustup")
        .arg("which")
        .args(["--toolchain", "nightly"])
        .arg("rustc")
        .output()?;
    let rustc_path = std::str::from_utf8(&output.stdout)?.trim();
    let json_path = PathBuf::from(rustc_path)
        .parent()
        .ok_or_else(|| anyhow!("could not get parent of {}", rustc_path))?
        .parent()
        .ok_or_else(|| anyhow!("could not get grandparent of {}", rustc_path))?
        .join("share/doc/rust/json");

    Ok(json_path)
}

pub fn collect<'a, N, T>(input: T) -> impl Iterator<Item = Vec<(&'a N,)>>
where
    N: 'a + Clone,
    T: Iterator<Item = (&'a N,)>,
{
    std::iter::once(input.collect::<Vec<_>>())
}

#[cfg(test)]
mod tests;
