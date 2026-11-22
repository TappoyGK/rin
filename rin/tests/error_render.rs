use rin::{Error, HeapAllocator, Limits, Machine, Span, Value, eval};
use std::fmt;

#[test]
fn render_keeps_header_and_adds_the_source_line() {
    let source = "(/ 1 0)";
    let error = eval(source).unwrap_err();
    let original = error.clone();
    assert_eq!(
        error.render(source),
        "1:1: division by zero [op.arith]\n  |\n1 | (/ 1 0)\n  | ^"
    );
    assert_eq!(error, original);
    assert_eq!(error.to_string(), "1:1: division by zero [op.arith]");
    let source = "(let (damage 1) damge)";
    let error = eval(source).unwrap_err();
    assert_eq!(
        error.render(source),
        "1:17: unknown name 'damge'; did you mean 'damage'? [expr.name]\n  |\n1 | (let (damage 1) damge)\n  |                 ^"
    );
}

#[test]
fn byte_columns_become_display_columns() {
    let source = "\"猫e\u{301}👨‍👩‍👧‍👦\"\t\"全角\"\tzzz";
    let error = eval(source).unwrap_err();
    assert_eq!(error.at.col, source.find("zzz").unwrap() + 1);
    assert!(
        error
            .render(source)
            .ends_with("\n  |\n1 | \"猫e\u{301}👨‍👩‍👧‍👦\" \"全角\"  zzz\n  |                 ^")
    );
    let source = "\t\tzzz";
    assert!(
        eval(source)
            .unwrap_err()
            .render(source)
            .ends_with("\n  |\n1 |         zzz\n  |         ^")
    );
}

#[test]
fn lines_eof_and_invalid_positions() {
    for (source, at, suffix) in [
        ("a\r\nb", Span::new(2, 2), "\n  |\n2 | b\n  |  ^"),
        ("a\r\nb", Span::new(1, 2), "\n  |\n1 | a\n  |  ^"),
        ("a\n", Span::new(2, 1), "\n  |\n2 | \n  | ^"),
        ("", Span::new(1, 1), "\n  |\n1 | \n  | ^"),
        ("a\r", Span::new(1, 2), "\n  |\n1 | a\n  |  ^"),
    ] {
        let error = Error::parse("src.syntax", at, "broken");
        assert_eq!(error.render(source), format!("{error}{suffix}"));
    }
    let source = format!("{}abc", "\n".repeat(99));
    let error = Error::parse("src.syntax", Span::new(100, 2), "broken");
    assert!(
        error
            .render(&source)
            .ends_with("\n    |\n100 | abc\n    |  ^")
    );
    for at in [
        Span::default(),
        Span::new(0, 1),
        Span::new(1, 0),
        Span::new(1, 2), // Inside the UTF-8 encoding of 猫.
        Span::new(1, 5),
        Span::new(2, 1),
        Span::new(usize::MAX, usize::MAX),
    ] {
        let error = Error::parse("src.syntax", at, "broken");
        assert_eq!(error.render("猫"), error.to_string(), "{at}");
    }
    let error = eval("(\n").unwrap_err();
    assert!(error.render("(\n").contains("2 | "));
}

#[test]
fn controls_are_escaped_using_the_same_width_for_the_caret() {
    let source = "a\0\u{1b}\r\u{2028}\t!";
    let error = Error::lex("src.token", Span::new(1, source.len()), "broken");
    assert!(
        error
            .render(source)
            .ends_with("\n  |\n1 | a\\u{0}\\u{1b}\\r\\u{2028}  !\n  |                         ^")
    );
}

#[test]
fn module_sources_follow_caller_locations() {
    let ma = "// ma\n(defn fail () (/ 1 0))";
    let mb = "// mb\n(import ma)\n(defn run () (+ 1 (ma.fail)))";
    let root = "(import mb)\n(mb.run)";
    let mut alloc = HeapAllocator;
    let mut machine = Machine::new(Limits::default(), &mut alloc);
    machine.load_module("ma", ma).unwrap();
    machine.load_module("mb", mb).unwrap();
    let error = machine.eval(root).unwrap_err();
    let mut requests = Vec::new();
    let rendered = error.render_with(|module| {
        requests.push(module.map(str::to_owned));
        match module {
            Some("ma") => Some(ma),
            Some("mb") => Some(mb),
            None => Some(root),
            _ => None,
        }
    });
    assert_eq!(requests, [Some("ma".into()), Some("mb".into()), None]);
    assert_eq!(rendered.matches(" |\n").count(), 3);
    assert!(rendered.contains("2 | (defn fail () (/ 1 0))"));
    assert!(rendered.contains("3 | (defn run () (+ 1 (ma.fail)))"));
    assert!(rendered.contains("2 | (mb.run)"));
    let single = error.render(ma);
    assert!(single.contains("2 | (defn fail () (/ 1 0))"));
    assert!(!single.contains("3 |"));
    assert_eq!(single.matches(" |\n").count(), 1);
    assert_eq!(error.render_with(|_| None), error.to_string());
    let partial = error.render_with(|module| (module == Some("mb")).then_some(mb));
    assert!(partial.contains("3 | (defn run () (+ 1 (ma.fail)))"));
    assert_eq!(partial.matches(" |\n").count(), 1);
}

#[test]
fn host_tail_builtin_and_compressed_frames_keep_their_meaning() {
    let mut alloc = HeapAllocator;
    let mut machine = Machine::new(Limits::default(), &mut alloc);
    let source = "(defn fail (x) (/ x 0))\n(defn tail (x) (fail x))";
    machine.load_module("ma", source).unwrap();
    let error = machine.call_in("ma", "tail", &[Value::Int(1)]).unwrap_err();
    let mut requests = 0;
    let rendered = error.render_with(|module| {
        requests += 1;
        assert_eq!(module, Some("ma"));
        Some(source)
    });
    assert_eq!(requests, 1); // No source for the host entry, including a tail call.
    assert!(rendered.contains("called from host) [tail calls elided: 1]"));
    let source = "(map (fn (x) (/ 1 x)) [0])";
    let error = eval(source).unwrap_err();
    assert!(error.render(source).contains("called by map at 1:1"));
    assert_eq!(error.render(source).matches(" |\n").count(), 2);
    let source = "(defn recur (n) (if (== n 0) (/ 1 0) (+ 1 (recur (- n 1)))))\n(recur 10)";
    let error = eval(source).unwrap_err();
    assert!(error.render(source).contains("×10"));
    assert_eq!(error.render_with(|_| None), error.to_string());
    let error = machine.call("not_defined", &[]).unwrap_err();
    assert_eq!(
        error.render_with(|_| panic!("host position has no source")),
        error.to_string()
    );
}

#[test]
fn writer_output_and_failures() {
    let source = "(/ 1 0)";
    let error = eval(source).unwrap_err();
    let mut out = "prefix\n".to_owned();
    error.render_to(&mut out, source).unwrap();
    assert_eq!(out, format!("prefix\n{}", error.render(source)));
    let mut out = String::new();
    let writer: &mut dyn fmt::Write = &mut out;
    error.render_with_to(writer, |_| Some(source)).unwrap();
    assert_eq!(out, error.render(source));
    struct Fails;
    impl fmt::Write for Fails {
        fn write_str(&mut self, _: &str) -> fmt::Result {
            Err(fmt::Error)
        }
    }
    assert!(
        error
            .render_with_to(&mut Fails, |_| panic!("must stop writing first"))
            .is_err()
    );
}
