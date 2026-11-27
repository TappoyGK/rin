//! ホスト境界（`bind` / `eval_with` / `call` / `call_in`）における実行時型適合検査 (`[type.boundary]`)。
//!
//! すべての作業スタック・訪問済み集合は `WorkArena` 上で確保し、システムヒープを使用しない。

use crate::ast::{TypeExpr, TypeExprKind};
use crate::error::{ErrRecorded, ErrorKind, Span};
use crate::mem::{AllocCore, RawVec, WorkArena};
use crate::pool::{Obj, Pool, RVal};

use super::types::{PermTypeNode, SchemaId, SchemaKind, TypeEnv, TypeId};
use super::work_types::oom;

#[derive(Clone, Copy)]
enum BoundaryTarget<'a> {
    Perm(TypeId),
    Expr(&'a TypeExpr),
}

/// ホスト境界に指定された `TypeId` が関数型（コンテナ・スキーマ内側を含む）を含まないことを検査する。
pub(crate) fn reject_fn_in_perm_type(
    root: TypeId,
    env: &TypeEnv,
    work: &mut WorkArena,
    core: &mut AllocCore<'_>,
    span: Span,
) -> Result<(), ErrRecorded> {
    let mut stack = RawVec::<TypeId>::new();
    let mut visited_schemas = RawVec::<SchemaId>::new();
    if stack.push_work(work, core, root).is_none() {
        return Err(oom(core, span));
    }
    while let Some(tid) = stack.pop() {
        match env.get_type(tid) {
            PermTypeNode::Fn { .. } => {
                return Err(core.err_str(
                    "type.boundary",
                    ErrorKind::Runtime,
                    span,
                    "function types are not allowed at host boundary",
                ));
            }
            PermTypeNode::List(t) | PermTypeNode::Array(t) | PermTypeNode::Option(t) => {
                if stack.push_work(work, core, t).is_none() {
                    return Err(oom(core, span));
                }
            }
            PermTypeNode::Result { ok, err } => {
                if stack.push_work(work, core, ok).is_none()
                    || stack.push_work(work, core, err).is_none()
                {
                    return Err(oom(core, span));
                }
            }
            PermTypeNode::Tuple(items) => {
                for &it in items.as_slice() {
                    if stack.push_work(work, core, it).is_none() {
                        return Err(oom(core, span));
                    }
                }
            }
            PermTypeNode::Schema(sid) => {
                if visited_schemas.as_slice().contains(&sid) {
                    continue;
                }
                if visited_schemas.push_work(work, core, sid).is_none() {
                    return Err(oom(core, span));
                }
                match env.get_schema(sid).kind {
                    SchemaKind::Struct { fields, .. } => {
                        for &(_, ft) in fields.as_slice() {
                            if stack.push_work(work, core, ft).is_none() {
                                return Err(oom(core, span));
                            }
                        }
                    }
                    SchemaKind::Variant { cases, .. } => {
                        for &c in cases.as_slice() {
                            if let Some(pt) = c.payload
                                && stack.push_work(work, core, pt).is_none()
                            {
                                return Err(oom(core, span));
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// ホスト境界の `TypeExpr` が関数型を含まず、参照型名が存在することを検査する（凍結環境対応）。
pub(crate) fn validate_boundary_type_expr(
    root: &TypeExpr,
    env: &TypeEnv,
    work: &mut WorkArena,
    core: &mut AllocCore<'_>,
) -> Result<(), ErrRecorded> {
    let mut stack = RawVec::<&TypeExpr>::new();
    if stack.push_work(work, core, root).is_none() {
        return Err(oom(core, root.span));
    }
    while let Some(te) = stack.pop() {
        match &te.kind {
            TypeExprKind::Fn { .. } => {
                return Err(core.err_str(
                    "type.boundary",
                    ErrorKind::Runtime,
                    te.span,
                    "function types are not allowed at host boundary",
                ));
            }
            TypeExprKind::List(t) | TypeExprKind::Array(t) | TypeExprKind::Option(t) => {
                if stack.push_work(work, core, t.as_ref()).is_none() {
                    return Err(oom(core, te.span));
                }
            }
            TypeExprKind::Result { ok, err } => {
                if stack.push_work(work, core, ok.as_ref()).is_none()
                    || stack.push_work(work, core, err.as_ref()).is_none()
                {
                    return Err(oom(core, te.span));
                }
            }
            TypeExprKind::Tuple(items) => {
                for it in items.as_slice() {
                    if stack.push_work(work, core, it).is_none() {
                        return Err(oom(core, it.span));
                    }
                }
            }
            TypeExprKind::Named { module, name } => {
                let sid = lookup_schema_in_env(env, *module, *name, core, te.span)?;
                let schema_ty_node_check = |env: &TypeEnv,
                                            sid: SchemaId,
                                            work: &mut WorkArena,
                                            core: &mut AllocCore<'_>,
                                            span: Span|
                 -> Result<(), ErrRecorded> {
                    let mut pstack = RawVec::<TypeId>::new();
                    let mut visited = RawVec::<SchemaId>::new();
                    if visited.push_work(work, core, sid).is_none() {
                        return Err(oom(core, span));
                    }
                    match env.get_schema(sid).kind {
                        SchemaKind::Struct { fields, .. } => {
                            for &(_, ft) in fields.as_slice() {
                                if pstack.push_work(work, core, ft).is_none() {
                                    return Err(oom(core, span));
                                }
                            }
                        }
                        SchemaKind::Variant { cases, .. } => {
                            for &c in cases.as_slice() {
                                if let Some(pt) = c.payload
                                    && pstack.push_work(work, core, pt).is_none()
                                {
                                    return Err(oom(core, span));
                                }
                            }
                        }
                    }
                    while let Some(tid) = pstack.pop() {
                        reject_fn_in_perm_type(tid, env, work, core, span)?;
                    }
                    Ok(())
                };
                schema_ty_node_check(env, sid, work, core, te.span)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn lookup_schema_in_env(
    env: &TypeEnv,
    module: Option<crate::mem::RawStr>,
    name: crate::mem::RawStr,
    core: &mut AllocCore<'_>,
    span: Span,
) -> Result<SchemaId, ErrRecorded> {
    if let Some(mname) = module {
        let Some((_, menv)) = env.lookup_module(mname.as_str()) else {
            return Err(core.err_str("expr.module", ErrorKind::Type, span, "unknown module"));
        };
        let h = crate::mem::fnv1a_bytes(name.as_bytes());
        menv.exports_types
            .get_by(h, |k| k.as_str() == name.as_str())
            .ok_or_else(|| {
                core.err_str(
                    "expr.module",
                    ErrorKind::Type,
                    span,
                    "module does not export this type",
                )
            })
    } else {
        env.lookup_root_type(name.as_str()).ok_or_else(|| {
            core.err_str("type.defschema", ErrorKind::Type, span, "unknown type name")
        })
    }
}

/// ホストから渡された `RVal` が `TypeId` に適合するか実行時検査する（`[type.boundary]`）。
pub(crate) fn check_rval_against_type(
    val: RVal,
    ty: TypeId,
    env: &TypeEnv,
    pool: &mut Pool<'_>,
    span: Span,
) -> Result<(), ErrRecorded> {
    check_rval_target(val, BoundaryTarget::Perm(ty), env, pool, span)
}

/// ホストから渡された `RVal` が `TypeExpr` に適合するか実行時検査する（凍結環境 `eval_with` 用）。
pub(crate) fn check_rval_against_type_expr(
    val: RVal,
    te: &TypeExpr,
    env: &TypeEnv,
    pool: &mut Pool<'_>,
    span: Span,
) -> Result<(), ErrRecorded> {
    check_rval_target(val, BoundaryTarget::Expr(te), env, pool, span)
}

fn check_rval_target<'a>(
    root_val: RVal,
    root_target: BoundaryTarget<'a>,
    env: &TypeEnv,
    pool: &mut Pool<'_>,
    span: Span,
) -> Result<(), ErrRecorded> {
    let mark = pool.work.mark(&pool.core);
    let res = (|| -> Result<(), ErrRecorded> {
        let mut stack = RawVec::<(RVal, BoundaryTarget<'a>)>::new();
        if stack
            .push_work(&mut pool.work, &mut pool.core, (root_val, root_target))
            .is_none()
        {
            return Err(oom(&mut pool.core, span));
        }

        while let Some((val, target)) = stack.pop() {
            match target {
                BoundaryTarget::Perm(tid) => match env.get_type(tid) {
                    PermTypeNode::Void => {
                        if !matches!(val, RVal::Void) {
                            return Err(boundary_err(&mut pool.core, span, "expected void"));
                        }
                    }
                    PermTypeNode::I64 => {
                        if !matches!(val, RVal::Int(_)) {
                            return Err(boundary_err(&mut pool.core, span, "expected i64"));
                        }
                    }
                    PermTypeNode::F64 => {
                        if !matches!(val, RVal::Float(_)) {
                            return Err(boundary_err(&mut pool.core, span, "expected f64"));
                        }
                    }
                    PermTypeNode::Bool => {
                        if !matches!(val, RVal::Bool(_)) {
                            return Err(boundary_err(&mut pool.core, span, "expected bool"));
                        }
                    }
                    PermTypeNode::Char => {
                        if !matches!(val, RVal::Char(_)) {
                            return Err(boundary_err(&mut pool.core, span, "expected char"));
                        }
                    }
                    PermTypeNode::Text => {
                        let ok =
                            matches!(val, RVal::Obj(id) if matches!(pool.get(id), Obj::Str(_)));
                        if !ok {
                            return Err(boundary_err(&mut pool.core, span, "expected text"));
                        }
                    }
                    PermTypeNode::Ident => {
                        let ok =
                            matches!(val, RVal::Obj(id) if matches!(pool.get(id), Obj::Ident(_)));
                        if !ok {
                            return Err(boundary_err(&mut pool.core, span, "expected ident"));
                        }
                    }
                    PermTypeNode::Datum => {
                        check_datum_rval(val, pool, &mut stack, BoundaryTarget::Perm(tid), span)?;
                    }
                    PermTypeNode::List(elem) => {
                        check_list_rval(val, pool, &mut stack, BoundaryTarget::Perm(elem), span)?;
                    }
                    PermTypeNode::Array(elem) => {
                        check_array_rval(val, pool, &mut stack, BoundaryTarget::Perm(elem), span)?;
                    }
                    PermTypeNode::Option(elem) => {
                        check_option_rval(val, pool, &mut stack, BoundaryTarget::Perm(elem), span)?;
                    }
                    PermTypeNode::Result { ok, err } => {
                        check_result_rval(
                            val,
                            pool,
                            &mut stack,
                            BoundaryTarget::Perm(ok),
                            BoundaryTarget::Perm(err),
                            span,
                        )?;
                    }
                    PermTypeNode::Tuple(items) => {
                        let RVal::Obj(id) = val else {
                            return Err(boundary_err(&mut pool.core, span, "expected tuple"));
                        };
                        let Obj::Tuple(vals) = *pool.get(id) else {
                            return Err(boundary_err(&mut pool.core, span, "expected tuple"));
                        };
                        if vals.len() != items.len() {
                            return Err(boundary_err(&mut pool.core, span, "tuple arity mismatch"));
                        }
                        for (&v, &t) in vals.as_slice().iter().zip(items.as_slice()) {
                            if stack
                                .push_work(
                                    &mut pool.work,
                                    &mut pool.core,
                                    (v, BoundaryTarget::Perm(t)),
                                )
                                .is_none()
                            {
                                return Err(oom(&mut pool.core, span));
                            }
                        }
                    }
                    PermTypeNode::Fn { .. } => {
                        return Err(boundary_err(
                            &mut pool.core,
                            span,
                            "function values are not allowed at host boundary",
                        ));
                    }
                    PermTypeNode::Schema(sid) => {
                        check_schema_rval(val, sid, env, pool, &mut stack, span)?;
                    }
                },
                BoundaryTarget::Expr(te) => match &te.kind {
                    TypeExprKind::Void => {
                        if !matches!(val, RVal::Void) {
                            return Err(boundary_err(&mut pool.core, span, "expected void"));
                        }
                    }
                    TypeExprKind::I64 => {
                        if !matches!(val, RVal::Int(_)) {
                            return Err(boundary_err(&mut pool.core, span, "expected i64"));
                        }
                    }
                    TypeExprKind::F64 => {
                        if !matches!(val, RVal::Float(_)) {
                            return Err(boundary_err(&mut pool.core, span, "expected f64"));
                        }
                    }
                    TypeExprKind::Bool => {
                        if !matches!(val, RVal::Bool(_)) {
                            return Err(boundary_err(&mut pool.core, span, "expected bool"));
                        }
                    }
                    TypeExprKind::Char => {
                        if !matches!(val, RVal::Char(_)) {
                            return Err(boundary_err(&mut pool.core, span, "expected char"));
                        }
                    }
                    TypeExprKind::Text => {
                        let ok =
                            matches!(val, RVal::Obj(id) if matches!(pool.get(id), Obj::Str(_)));
                        if !ok {
                            return Err(boundary_err(&mut pool.core, span, "expected text"));
                        }
                    }
                    TypeExprKind::Ident => {
                        let ok =
                            matches!(val, RVal::Obj(id) if matches!(pool.get(id), Obj::Ident(_)));
                        if !ok {
                            return Err(boundary_err(&mut pool.core, span, "expected ident"));
                        }
                    }
                    TypeExprKind::Datum => {
                        check_datum_rval(val, pool, &mut stack, BoundaryTarget::Expr(te), span)?;
                    }
                    TypeExprKind::List(elem) => {
                        check_list_rval(
                            val,
                            pool,
                            &mut stack,
                            BoundaryTarget::Expr(elem.as_ref()),
                            span,
                        )?;
                    }
                    TypeExprKind::Array(elem) => {
                        check_array_rval(
                            val,
                            pool,
                            &mut stack,
                            BoundaryTarget::Expr(elem.as_ref()),
                            span,
                        )?;
                    }
                    TypeExprKind::Option(elem) => {
                        check_option_rval(
                            val,
                            pool,
                            &mut stack,
                            BoundaryTarget::Expr(elem.as_ref()),
                            span,
                        )?;
                    }
                    TypeExprKind::Result { ok, err } => {
                        check_result_rval(
                            val,
                            pool,
                            &mut stack,
                            BoundaryTarget::Expr(ok.as_ref()),
                            BoundaryTarget::Expr(err.as_ref()),
                            span,
                        )?;
                    }
                    TypeExprKind::Tuple(items) => {
                        let RVal::Obj(id) = val else {
                            return Err(boundary_err(&mut pool.core, span, "expected tuple"));
                        };
                        let Obj::Tuple(vals) = *pool.get(id) else {
                            return Err(boundary_err(&mut pool.core, span, "expected tuple"));
                        };
                        if vals.len() != items.len() {
                            return Err(boundary_err(&mut pool.core, span, "tuple arity mismatch"));
                        }
                        for (&v, t) in vals.as_slice().iter().zip(items.as_slice()) {
                            if stack
                                .push_work(
                                    &mut pool.work,
                                    &mut pool.core,
                                    (v, BoundaryTarget::Expr(t)),
                                )
                                .is_none()
                            {
                                return Err(oom(&mut pool.core, span));
                            }
                        }
                    }
                    TypeExprKind::Fn { .. } => {
                        return Err(boundary_err(
                            &mut pool.core,
                            span,
                            "function values are not allowed at host boundary",
                        ));
                    }
                    TypeExprKind::Named { module, name } => {
                        let sid = lookup_schema_in_env(env, *module, *name, &mut pool.core, span)?;
                        check_schema_rval(val, sid, env, pool, &mut stack, span)?;
                    }
                },
            }
        }
        Ok(())
    })();
    pool.work.rewind(&mut pool.core, mark);
    res
}

fn check_datum_rval<'a>(
    val: RVal,
    pool: &mut Pool<'_>,
    stack: &mut RawVec<(RVal, BoundaryTarget<'a>)>,
    datum_target: BoundaryTarget<'a>,
    span: Span,
) -> Result<(), ErrRecorded> {
    match val {
        RVal::Int(_)
        | RVal::Float(_)
        | RVal::Bool(_)
        | RVal::Char(_)
        | RVal::None
        | RVal::EmptyList => Ok(()),
        RVal::Obj(id) => match *pool.get(id) {
            Obj::Str(_) | Obj::Ident(_) => Ok(()),
            Obj::Cons { .. } => check_list_rval(val, pool, stack, datum_target, span),
            _ => Err(boundary_err(&mut pool.core, span, "expected datum")),
        },
        _ => Err(boundary_err(&mut pool.core, span, "expected datum")),
    }
}

fn check_list_rval<'a>(
    mut cur: RVal,
    pool: &mut Pool<'_>,
    stack: &mut RawVec<(RVal, BoundaryTarget<'a>)>,
    elem_target: BoundaryTarget<'a>,
    span: Span,
) -> Result<(), ErrRecorded> {
    loop {
        match cur {
            RVal::EmptyList => return Ok(()),
            RVal::Obj(id) => {
                let Obj::Cons { head, tail, .. } = *pool.get(id) else {
                    return Err(boundary_err(&mut pool.core, span, "expected list"));
                };
                if stack
                    .push_work(&mut pool.work, &mut pool.core, (head, elem_target))
                    .is_none()
                {
                    return Err(oom(&mut pool.core, span));
                }
                cur = tail;
            }
            _ => return Err(boundary_err(&mut pool.core, span, "expected list")),
        }
    }
}

fn check_array_rval<'a>(
    val: RVal,
    pool: &mut Pool<'_>,
    stack: &mut RawVec<(RVal, BoundaryTarget<'a>)>,
    elem_target: BoundaryTarget<'a>,
    span: Span,
) -> Result<(), ErrRecorded> {
    let RVal::Obj(id) = val else {
        return Err(boundary_err(&mut pool.core, span, "expected array"));
    };
    let Obj::Array(items) = *pool.get(id) else {
        return Err(boundary_err(&mut pool.core, span, "expected array"));
    };
    for &item in items.as_slice() {
        if stack
            .push_work(&mut pool.work, &mut pool.core, (item, elem_target))
            .is_none()
        {
            return Err(oom(&mut pool.core, span));
        }
    }
    Ok(())
}

fn check_option_rval<'a>(
    val: RVal,
    pool: &mut Pool<'_>,
    stack: &mut RawVec<(RVal, BoundaryTarget<'a>)>,
    elem_target: BoundaryTarget<'a>,
    span: Span,
) -> Result<(), ErrRecorded> {
    match val {
        RVal::None => Ok(()),
        RVal::Obj(id) => {
            let Obj::Some(inner) = *pool.get(id) else {
                return Err(boundary_err(&mut pool.core, span, "expected option"));
            };
            if stack
                .push_work(&mut pool.work, &mut pool.core, (inner, elem_target))
                .is_none()
            {
                return Err(oom(&mut pool.core, span));
            }
            Ok(())
        }
        _ => Err(boundary_err(&mut pool.core, span, "expected option")),
    }
}

fn check_result_rval<'a>(
    val: RVal,
    pool: &mut Pool<'_>,
    stack: &mut RawVec<(RVal, BoundaryTarget<'a>)>,
    ok_target: BoundaryTarget<'a>,
    err_target: BoundaryTarget<'a>,
    span: Span,
) -> Result<(), ErrRecorded> {
    let RVal::Obj(id) = val else {
        return Err(boundary_err(&mut pool.core, span, "expected result"));
    };
    match *pool.get(id) {
        Obj::Ok(inner) => {
            if stack
                .push_work(&mut pool.work, &mut pool.core, (inner, ok_target))
                .is_none()
            {
                return Err(oom(&mut pool.core, span));
            }
            Ok(())
        }
        Obj::Err(inner) => {
            if stack
                .push_work(&mut pool.work, &mut pool.core, (inner, err_target))
                .is_none()
            {
                return Err(oom(&mut pool.core, span));
            }
            Ok(())
        }
        _ => Err(boundary_err(&mut pool.core, span, "expected result")),
    }
}

fn check_schema_rval<'a>(
    val: RVal,
    sid: SchemaId,
    env: &TypeEnv,
    pool: &mut Pool<'_>,
    stack: &mut RawVec<(RVal, BoundaryTarget<'a>)>,
    span: Span,
) -> Result<(), ErrRecorded> {
    let def = *env.get_schema(sid);
    match def.kind {
        SchemaKind::Struct {
            open,
            type_name,
            fields,
        } => {
            let RVal::Obj(id) = val else {
                return Err(boundary_err(&mut pool.core, span, "expected struct"));
            };
            let Obj::Struct { desc, values } = *pool.get(id) else {
                return Err(boundary_err(&mut pool.core, span, "expected struct"));
            };
            if pool.struct_desc_name(desc) != type_name.as_str() {
                return Err(boundary_err(
                    &mut pool.core,
                    span,
                    "struct type name mismatch",
                ));
            }
            let val_field_count = pool.struct_desc_field_count(desc);
            let val_slice = values.as_slice();
            if !open {
                for i in 0..val_field_count {
                    let vf = pool.struct_desc_field_name(desc, i);
                    if !fields.as_slice().iter().any(|(fn_, _)| fn_.as_str() == vf) {
                        return Err(boundary_err(
                            &mut pool.core,
                            span,
                            "unexpected extra field in struct",
                        ));
                    }
                }
            }
            for &(fname, fty) in fields.as_slice() {
                let Some(idx) = pool.struct_desc_find_field(desc, fname.as_str()) else {
                    return Err(boundary_err(
                        &mut pool.core,
                        span,
                        "missing required struct field",
                    ));
                };
                let fv = val_slice[idx];
                if stack
                    .push_work(
                        &mut pool.work,
                        &mut pool.core,
                        (fv, BoundaryTarget::Perm(fty)),
                    )
                    .is_none()
                {
                    return Err(oom(&mut pool.core, span));
                }
            }
            Ok(())
        }
        SchemaKind::Variant { type_name, cases } => {
            let RVal::Obj(id) = val else {
                return Err(boundary_err(&mut pool.core, span, "expected variant"));
            };
            let Obj::Variant {
                type_name: vtn,
                variant,
                data,
            } = *pool.get(id)
            else {
                return Err(boundary_err(&mut pool.core, span, "expected variant"));
            };
            if vtn.as_str() != type_name.as_str() {
                return Err(boundary_err(
                    &mut pool.core,
                    span,
                    "variant type name mismatch",
                ));
            }
            let Some(case) = cases
                .as_slice()
                .iter()
                .find(|c| c.name.as_str() == variant.as_str())
            else {
                return Err(boundary_err(&mut pool.core, span, "unknown variant case"));
            };
            match (case.payload, data) {
                (Some(pty), Some(dv)) => {
                    if stack
                        .push_work(
                            &mut pool.work,
                            &mut pool.core,
                            (dv, BoundaryTarget::Perm(pty)),
                        )
                        .is_none()
                    {
                        return Err(oom(&mut pool.core, span));
                    }
                    Ok(())
                }
                (None, None) => Ok(()),
                (Some(_), None) => Err(boundary_err(
                    &mut pool.core,
                    span,
                    "variant case requires payload",
                )),
                (None, Some(_)) => Err(boundary_err(
                    &mut pool.core,
                    span,
                    "variant case does not accept payload",
                )),
            }
        }
    }
}

#[inline]
fn boundary_err(core: &mut AllocCore<'_>, span: Span, msg: &str) -> ErrRecorded {
    core.err_str("type.boundary", ErrorKind::Runtime, span, msg)
}
