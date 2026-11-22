//! コメントとリテラルを保持する、設定不要のソース整形。
mod cst;
mod syntax;

use cst::{Cst, Output};

use crate::ast::Expr;
use crate::lexer::tokenize_lossless;
use crate::parser::{parse_owned, parse_tests};
use crate::{Error, Span};
use syntax::{Syntax, equivalent};

const WIDTH: usize = 100;

/// ソースを2桁字下げ・100文字幅で整形する。
/// 構文エラー、または整形前後のASTが異なる場合はエラーを返す。
/// リテラル内部は原文を保ち、コメントの前後にあるトークンの順序を変えない。
pub fn format(source: &str) -> Result<String, Error> {
    let before = parse(source)?;
    let tokens = tokenize_lossless(source)?;
    let syntax = Syntax::new(&tokens, &before);
    let cst = Cst::build(source, &syntax);
    drop(syntax);
    let mut out = Output::default();
    cst.render_root(&mut out);
    let output = out.finish();
    drop(cst);
    drop(tokens);
    let after = parse(&output).map_err(|_| {
        Error::parse(
            "fmt.preserve",
            Span::default(),
            "formatting changed the source syntax",
        )
    })?;
    if !equivalent(&before, &after) {
        return Err(Error::parse(
            "fmt.preserve",
            Span::default(),
            "formatting changed the source AST",
        ));
    }
    Ok(output)
}

fn parse(source: &str) -> Result<Vec<Expr>, Error> {
    match parse_owned(source, false) {
        Ok(exprs) => Ok(exprs),
        Err(error) if matches!(error.rule(), "test.deftest" | "test.check") => {
            crate::test::validate_source(source)?;
            parse_tests(source)
        }
        Err(error) => Err(error),
    }
}
