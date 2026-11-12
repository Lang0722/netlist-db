use crate::{Subckt, instance::InstanceCtx};
use indexmap::IndexMap;
use std::{collections::HashMap, iter::once, ptr};

/// [`Subckt::append`] follows the instance hierarchy at most this deep
pub const MAX_HIERARCHY_DEPTH: usize = 256;

impl<'s> Subckt<'s> {
    /// Walk the hierarchy under the subckt `cktname`: collect its internal
    /// nodes (with their instance paths) and the top-level subckts it uses.
    /// This walk does not look inside `.if` blocks.
    ///
    /// Errors: the instance path of a subckt that is not defined, or of a
    /// subckt that instantiates itself (directly or through other subckts),
    /// which used to recurse until the stack overflowed.
    pub fn append<'a>(
        cktname: &str,
        need_skip: impl Copy + Fn(&str) -> bool,
        inst_path: &mut Vec<&'a &'s str>,
        include_subckts: &mut Vec<&'a IndexMap<String, Self>>,
        internal_nodes: &mut Vec<String>,
        append_subckts: &mut Vec<&'a Self>,
    ) -> Result<(), String> {
        Self::append_inner(
            cktname,
            need_skip,
            inst_path,
            include_subckts,
            internal_nodes,
            append_subckts,
            &mut Vec::new(),
        )
    }
    /// `expanding`: the subckts being expanded, from the top down
    fn append_inner<'a>(
        cktname: &str,
        need_skip: impl Copy + Fn(&str) -> bool,
        inst_path: &mut Vec<&'a &'s str>,
        include_subckts: &mut Vec<&'a IndexMap<String, Self>>,
        internal_nodes: &mut Vec<String>,
        append_subckts: &mut Vec<&'a Self>,
        expanding: &mut Vec<&'a Self>,
    ) -> Result<(), String> {
        if need_skip(cktname) {
            return Ok(());
        }
        let path = |inst_path: &[&&'s str]| {
            inst_path
                .iter()
                .map(|inst| **inst)
                .chain(once(cktname))
                .collect::<Vec<_>>()
                .join(".")
        };
        for (idx, include) in include_subckts.iter().enumerate().rev() {
            if let Some(ckt) = include.get(&cktname.to_lowercase()) {
                if let Some(pos) = expanding.iter().position(|s| ptr::eq(*s, ckt)) {
                    let cycle = expanding[pos..]
                        .iter()
                        .map(|s| s.name)
                        .chain(once(ckt.name))
                        .collect::<Vec<_>>()
                        .join(" -> ");
                    return Err(format!(
                        "{}: subckt `{}` instantiates itself ({cycle})",
                        path(inst_path),
                        ckt.name
                    ));
                }
                if expanding.len() >= MAX_HIERARCHY_DEPTH {
                    return Err(format!(
                        "{}: the hierarchy is more than {MAX_HIERARCHY_DEPTH} levels deep",
                        path(inst_path)
                    ));
                }
                let mut nodes_map =
                    ckt.ast
                        .instance
                        .iter()
                        .fold(HashMap::new(), |mut nodes_map, inst| {
                            inst.ctx.append_nodes(&mut nodes_map);
                            nodes_map
                        });
                for port in &ckt.ports {
                    _ = nodes_map.remove(&port.to_lowercase());
                }
                internal_nodes.extend(nodes_map.into_values().map(|n| {
                    let path = inst_path.iter().copied().chain(once(n));
                    path.copied().collect::<Vec<_>>().join(".")
                }));
                if idx == 0 {
                    // idx==0 means in top
                    append_subckts.push(ckt);
                }
                include_subckts.push(&ckt.ast.subckt);
                expanding.push(ckt);
                for inst in &ckt.ast.instance {
                    if let InstanceCtx::Subckt(ckt) = &inst.ctx {
                        inst_path.push(&inst.name);
                        Self::append_inner(
                            ckt.cktname,
                            need_skip,
                            inst_path,
                            include_subckts,
                            internal_nodes,
                            append_subckts,
                            expanding,
                        )?;
                        inst_path.pop();
                    }
                }
                expanding.pop();
                include_subckts.pop();
                return Ok(());
            }
        }
        Err(path(inst_path))
    }
}

#[cfg(test)]
pub(crate) mod test {
    use std::path::PathBuf;

    use crate::{FileId, Subckt, parser::parse_top_multi};

    /// Prints the messages of the library
    #[cfg(not(feature = "tracing"))]
    struct Messages;

    #[cfg(not(feature = "tracing"))]
    impl log::Log for Messages {
        fn enabled(&self, _: &log::Metadata<'_>) -> bool {
            true
        }
        fn log(&self, record: &log::Record<'_>) {
            let (level, target) = (record.level(), record.target());
            println!("{level:<5} [{target}] {}", record.args());
        }
        fn flush(&self) {}
    }

    pub(crate) fn init_logger() {
        #[cfg(not(feature = "tracing"))]
        if log::set_logger(&Messages).is_ok() {
            log::set_max_level(log::LevelFilter::Trace);
        }
        #[cfg(feature = "tracing")]
        {
            let subscriber = tracing_subscriber::FmtSubscriber::builder()
                .with_max_level(tracing::Level::DEBUG)
                .with_target(false)
                .with_file(true)
                .with_line_number(true)
                .finish();
            _ = tracing::subscriber::set_global_default(subscriber);
        }
    }

    #[tokio::test]
    async fn test_append() {
        init_logger();
        let (parsed, files) = parse_top_multi(
            [FileId::Include {
                path: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/async_sync_dff.cdl"),
            }]
            .into_iter(),
        )
        .await;
        let (ast, has_err) = files.build(parsed);
        assert!(!has_err);
        let mut internal_nodes = Vec::new();
        let mut append_subckts = Vec::new();
        Subckt::append(
            "ASYNC_SYNC_DFF",
            |_s| false,
            &mut Vec::new(),
            &mut vec![&ast.subckt],
            &mut internal_nodes,
            &mut append_subckts,
        )
        .unwrap();
        dbg!(internal_nodes);
        for append_subckt in append_subckts {
            println!("{append_subckt}");
        }
    }
}
