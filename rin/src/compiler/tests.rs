use super::*;
use crate::ErrorKind;

fn compile(source: &str) -> Chunk {
    let ast = crate::parser::parse_owned(source, false).unwrap();
    compile_program(ast.exprs(), None, &mut SymTable::default(), &[], false).unwrap()
}

#[test]
fn sparse_diagnostics_do_not_retain_capacity_for_unrelated_ast() {
    for width in [0, 20, 2000] {
        let source = format!("{}(let (damage 1) damge)", "(abs 1) ".repeat(width));
        let chunk = compile(&source);
        let table = chunk.names.as_ref().unwrap().table.get().unwrap();
        let bytes = table.text.capacity()
            + table.nodes.capacity() * std::mem::size_of::<NameNode>()
            + table.sites.capacity() * std::mem::size_of::<NameSite>();
        assert!(bytes <= 512, "width={width}, retained bytes={bytes}");
        assert_eq!(table.nodes.len(), 1);
        assert_eq!(table.sites.len(), 1);
    }
}

#[test]
fn names_are_only_kept_at_global_lookups_and_shared_between_closures() {
    for source in [
        "(let (damage 7) damage)",
        "(let (damage 7) (fn () damage))",
        // `*` は GetGlobal だが、2 文字以下には候補を表示しない。
        "(let (damage 7) (* damage damage))",
        "(let (damage 7) (fn (x) (* damage x)))",
    ] {
        let chunk = compile(source);
        assert!(chunk.names.is_none());
        assert!(chunk.protos.iter().all(|proto| proto.chunk.names.is_none()));
    }
    for depth in [0, 12] {
        let bindings = (0..20).map(|i| format!("local{i} 1 ")).collect::<String>();
        let children = "(fn (argument) missing) ".repeat(20);
        let source = format!(
            "{}(let ({bindings}) {children} 0){}",
            "(let () ".repeat(depth),
            ")".repeat(depth)
        );
        let chunk = compile(&source);
        // 親自身に GetGlobal がなくても、子は必要な外側の束縛を参照できる。
        assert!(chunk.names.is_none());
        let first = chunk.protos[0].chunk.names.as_ref().unwrap();
        assert_eq!(first.table.get().unwrap().nodes.len(), 40); // 外側 20 + 各関数の引数 1
        assert_eq!(chunk.protos.len(), 20);
        for proto in &chunk.protos {
            let names = proto.chunk.names.as_ref().unwrap();
            assert!(Rc::ptr_eq(&names.table, &first.table));
            let ip = names.table.get().unwrap().sites[names.last_site].ip;
            assert_eq!(proto.chunk.visible_names(ip).count(), 21);
        }
    }
}

#[test]
fn lookup_sites_keep_their_own_scope_snapshot() {
    for depth in [0, 12] {
        let source = format!(
            "{}(let (damage 1) known (let (health 2) known)) (let (armor 3) known){}",
            "(let () ".repeat(depth),
            ")".repeat(depth)
        );
        let chunk = compile(&source);
        let names = chunk.names.as_ref().unwrap();
        let table = names.table.get().unwrap();
        let mut sites = Vec::new();
        let mut site = Some(names.last_site);
        while let Some(index) = site {
            let entry = &table.sites[index];
            sites.push((entry.ip, entry.head));
            site = entry.previous;
        }
        sites.reverse();
        assert_eq!(sites.len(), 3);
        assert_eq!(names.table.get().unwrap().nodes.len(), 3);
        for ((ip, _), expected) in
            sites
                .into_iter()
                .zip([vec!["damage"], vec!["health", "damage"], vec!["armor"]])
        {
            assert_eq!(chunk.visible_names(ip).collect::<Vec<_>>(), expected);
        }
        assert_eq!(chunk.visible_names(usize::MAX).count(), 0);
    }
}

#[test]
fn nested_name_nodes_grow_linearly() {
    let depth = 30;
    let source = format!(
        "{}0{}",
        "(fn (a b c d e) missing ".repeat(depth),
        ")".repeat(depth)
    );
    let chunk = compile(&source);
    let mut current = &chunk;
    let first = chunk.protos[0].chunk.names.as_ref().unwrap();
    assert_eq!(first.table.get().unwrap().nodes.len(), depth * 5);
    for _ in 0..depth {
        current = &current.protos[0].chunk;
        let names = current.names.as_ref().unwrap();
        assert!(Rc::ptr_eq(&names.table, &first.table));
    }
}

#[test]
fn local_slot_overflow_reports_capacity_rule() {
    // 巨大なソースを確保せず、個数上限の直前の状態から検査する。
    let mut scope = Scope::new(None);
    scope.next_slot = u32::MAX - 1;
    let at = Span::new(3, 5);
    assert_eq!(scope.alloc_slot("last", at).unwrap(), u32::MAX - 1);
    let err = scope.alloc_slot("overflow", at).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Parse);
    assert_eq!(err.rule(), "compile.capacity");
    assert_eq!(err.at, at);
}
