//! 公開の型ハンドル (`TypeHandle`)・型参照 (`TypeRef`)・関数シグネチャ参照 (`FnSigRef`)。
//!
//! すべて `PermArena` 上の永続型環境 (`TypeEnv`) をゼロコピー・ゼロヒープで参照する。

use std::fmt;

use super::types::{FnSig, PermTypeNode, TypeEnv, TypeId};

/// 事前解決済みのホスト境界用型ハンドル。
///
/// [`crate::Builder::resolve_type`] で取得し、[`crate::Builder::bind_typed`] や
/// [`crate::Machine::eval_with_typed_vals`] / [`crate::Machine::eval_with_typed`] に渡すことで、
/// 反復呼び出し時の型式文字列パースを省略できる。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeHandle(pub(crate) TypeId);

impl TypeHandle {
    #[inline]
    pub(crate) const fn from_raw(id: u32) -> Self {
        Self(TypeId(id))
    }

    #[inline]
    pub(crate) const fn raw(self) -> u32 {
        self.0.0
    }
}

/// 永続型環境 (`TypeEnv`) 内の解決済み型への借用ビュー（ゼロヒープ）。
#[derive(Clone, Copy)]
pub struct TypeRef<'a> {
    env: &'a TypeEnv,
    id: TypeId,
}

impl<'a> TypeRef<'a> {
    #[inline]
    pub(crate) fn new(env: &'a TypeEnv, id: TypeId) -> Self {
        Self { env, id }
    }
}

impl fmt::Display for TypeRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_perm_type(self.env, self.id, f)
    }
}

impl fmt::Debug for TypeRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_perm_type(self.env, self.id, f)
    }
}

/// 定義済み関数 (`defn`) のシグネチャへの借用ビュー（ゼロヒープ）。
#[derive(Clone, Copy)]
pub struct FnSigRef<'a> {
    env: &'a TypeEnv,
    sig: FnSig,
}

impl<'a> FnSigRef<'a> {
    #[inline]
    pub(crate) fn new(env: &'a TypeEnv, sig: FnSig) -> Self {
        Self { env, sig }
    }

    /// 仮引数の個数を返す。
    #[inline]
    pub fn param_count(&self) -> usize {
        self.sig.params.len()
    }

    /// `index` 番目の仮引数名を返す。
    #[inline]
    pub fn param_name(&self, index: usize) -> Option<&'a str> {
        let name = self.sig.params.as_slice().get(index)?.0;
        // SAFETY: `RawStr` は `PermArena` 上の不変文字列であり、`&'a TypeEnv` の寿命 `'a` の間有効。
        Some(unsafe { std::mem::transmute::<&str, &'a str>(name.as_str()) })
    }

    /// `index` 番目の仮引数型を返す。
    #[inline]
    pub fn param_type(&self, index: usize) -> Option<TypeRef<'a>> {
        self.sig
            .params
            .as_slice()
            .get(index)
            .map(|(_, tid)| TypeRef::new(self.env, *tid))
    }

    /// 戻り値型を返す。
    #[inline]
    pub fn ret_type(&self) -> TypeRef<'a> {
        TypeRef::new(self.env, self.sig.ret)
    }
}

impl fmt::Display for FnSigRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("(")?;
        for (i, &(pname, ptid)) in self.sig.params.as_slice().iter().enumerate() {
            if i > 0 {
                f.write_str(" ")?;
            }
            write!(f, "({} ", pname.as_str())?;
            write_perm_type(self.env, ptid, f)?;
            f.write_str(")")?;
        }
        f.write_str(") -> ")?;
        write_perm_type(self.env, self.sig.ret, f)
    }
}

impl fmt::Debug for FnSigRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// 永続型ノードを Rin の正規型式表記で書き出す（深さは構文上限 256 段以内）。
pub(crate) fn write_perm_type(env: &TypeEnv, id: TypeId, f: &mut impl fmt::Write) -> fmt::Result {
    match env.get_type(id) {
        PermTypeNode::Void => f.write_str("void"),
        PermTypeNode::I64 => f.write_str("i64"),
        PermTypeNode::F64 => f.write_str("f64"),
        PermTypeNode::Bool => f.write_str("bool"),
        PermTypeNode::Char => f.write_str("char"),
        PermTypeNode::Text => f.write_str("text"),
        PermTypeNode::Ident => f.write_str("ident"),
        PermTypeNode::Datum => f.write_str("datum"),
        PermTypeNode::List(inner) => {
            f.write_str("(list ")?;
            write_perm_type(env, inner, f)?;
            f.write_str(")")
        }
        PermTypeNode::Array(inner) => {
            f.write_str("(array ")?;
            write_perm_type(env, inner, f)?;
            f.write_str(")")
        }
        PermTypeNode::Option(inner) => {
            f.write_str("(option ")?;
            write_perm_type(env, inner, f)?;
            f.write_str(")")
        }
        PermTypeNode::Result { ok, err } => {
            f.write_str("(result ")?;
            write_perm_type(env, ok, f)?;
            f.write_str(" ")?;
            write_perm_type(env, err, f)?;
            f.write_str(")")
        }
        PermTypeNode::Tuple(items) => {
            f.write_str("(tuple")?;
            for &it in items.as_slice() {
                f.write_str(" ")?;
                write_perm_type(env, it, f)?;
            }
            f.write_str(")")
        }
        PermTypeNode::Fn { params, ret } => {
            f.write_str("(fn (")?;
            for (i, &p) in params.as_slice().iter().enumerate() {
                if i > 0 {
                    f.write_str(" ")?;
                }
                write_perm_type(env, p, f)?;
            }
            f.write_str(") ")?;
            write_perm_type(env, ret, f)?;
            f.write_str(")")
        }
        PermTypeNode::Schema(sid) => {
            let def = env.get_schema(sid);
            if let Some(m) = def.module {
                write!(f, "{}.{}", m.as_str(), def.name.as_str())
            } else {
                f.write_str(def.name.as_str())
            }
        }
    }
}
