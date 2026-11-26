//! システムヒープを使わずプール上の値を直接読み取る借用ビュー [`ValueRef`] と値ハンドル [`Val`] を提供する。

use std::fmt::{self, Write};

use crate::pool::{Obj, Pool, RVal, StructDescRef};

/// [`ValueRef`] が指す値の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueTag {
    Void,
    Int,
    Float,
    Bool,
    Char,
    Str,
    List,
    Array,
    Tuple,
    Struct,
    Variant,
    None,
    Some,
    Ok,
    Err,
    Fn,
    Ident,
}

/// [`crate::Builder`] / [`crate::Machine`] のアロケータ上に確保された値への不透明ハンドル。
///
/// 使い終わったら同じマシンの `release_val` へ返却する。
#[derive(Debug, Clone, Copy)]
pub struct Val(pub(crate) RVal);

impl Val {
    #[inline]
    pub(crate) const fn void() -> Self {
        Self(RVal::Void)
    }

    #[inline]
    pub(crate) const fn empty_list() -> Self {
        Self(RVal::EmptyList)
    }
}

/// プール上の値をシステムヒープへ複製せず直接読み取る借用ビュー。
#[derive(Clone, Copy)]
pub struct ValueRef<'a> {
    pub(crate) val: RVal,
    pub(crate) pool: &'a Pool<'a>,
}

impl<'a> ValueRef<'a> {
    #[inline]
    pub(crate) fn new(val: RVal, pool: &'a Pool<'_>) -> Self {
        // SAFETY: `Pool<'p>` のライフタイム `'p` は共変的な共有参照として `'a` に縮小できる。
        let pool: &'a Pool<'a> = unsafe { std::mem::transmute(pool) };
        Self { val, pool }
    }

    pub fn tag(&self) -> ValueTag {
        match self.val {
            RVal::Void => ValueTag::Void,
            RVal::Int(_) => ValueTag::Int,
            RVal::Float(_) => ValueTag::Float,
            RVal::Bool(_) => ValueTag::Bool,
            RVal::Char(_) => ValueTag::Char,
            RVal::None => ValueTag::None,
            RVal::EmptyList => ValueTag::List,
            RVal::Builtin(_) => ValueTag::Fn,
            RVal::Obj(id) => match self.pool.get(id) {
                Obj::Str(_) => ValueTag::Str,
                Obj::Ident(_) => ValueTag::Ident,
                Obj::Cons { .. } => ValueTag::List,
                Obj::Array(_) => ValueTag::Array,
                Obj::Tuple(_) => ValueTag::Tuple,
                Obj::Struct { .. } => ValueTag::Struct,
                Obj::Variant { .. } => ValueTag::Variant,
                Obj::Some(_) => ValueTag::Some,
                Obj::Ok(_) => ValueTag::Ok,
                Obj::Err(_) => ValueTag::Err,
                Obj::Closure { .. } | Obj::Module { .. } | Obj::StructDesc { .. } => ValueTag::Fn,
            },
        }
    }

    #[inline]
    pub fn is_void(&self) -> bool {
        matches!(self.val, RVal::Void)
    }

    #[inline]
    pub fn is_none(&self) -> bool {
        matches!(self.val, RVal::None)
    }

    #[inline]
    pub fn as_int(&self) -> Option<i64> {
        match self.val {
            RVal::Int(n) => Some(n),
            _ => None,
        }
    }

    #[inline]
    pub fn as_float(&self) -> Option<f64> {
        match self.val {
            RVal::Float(x) => Some(x),
            _ => None,
        }
    }

    #[inline]
    pub fn as_bool(&self) -> Option<bool> {
        match self.val {
            RVal::Bool(b) => Some(b),
            _ => None,
        }
    }

    #[inline]
    pub fn as_char(&self) -> Option<char> {
        match self.val {
            RVal::Char(c) => Some(c),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&'a str> {
        match self.val {
            RVal::Obj(id) => match self.pool.get(id) {
                Obj::Str(s) => Some(s.as_str()),
                _ => None,
            },
            _ => None,
        }
    }

    pub fn as_ident(&self) -> Option<&'a str> {
        match self.val {
            RVal::Obj(id) => match self.pool.get(id) {
                Obj::Ident(s) => Some(s.as_str()),
                _ => None,
            },
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<ListIter<'a>> {
        match self.val {
            RVal::EmptyList => Some(ListIter {
                cur: RVal::EmptyList,
                pool: self.pool,
            }),
            RVal::Obj(id) if matches!(self.pool.get(id), Obj::Cons { .. }) => Some(ListIter {
                cur: self.val,
                pool: self.pool,
            }),
            _ => None,
        }
    }

    pub fn list_len(&self) -> Option<usize> {
        match self.val {
            RVal::EmptyList => Some(0),
            RVal::Obj(id) => match self.pool.get(id) {
                Obj::Cons { len, .. } => Some(*len),
                _ => None,
            },
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<SliceView<'a>> {
        match self.val {
            RVal::Obj(id) => match self.pool.get(id) {
                Obj::Array(xs) => Some(SliceView {
                    items: xs.as_slice(),
                    pool: self.pool,
                }),
                _ => None,
            },
            _ => None,
        }
    }

    pub fn as_tuple(&self) -> Option<SliceView<'a>> {
        match self.val {
            RVal::Obj(id) => match self.pool.get(id) {
                Obj::Tuple(xs) => Some(SliceView {
                    items: xs.as_slice(),
                    pool: self.pool,
                }),
                _ => None,
            },
            _ => None,
        }
    }

    pub fn as_struct(&self) -> Option<StructRef<'a>> {
        match self.val {
            RVal::Obj(id) => match self.pool.get(id) {
                Obj::Struct { desc, values } => Some(StructRef {
                    desc: *desc,
                    values: values.as_slice(),
                    pool: self.pool,
                }),
                _ => None,
            },
            _ => None,
        }
    }

    pub fn as_variant(&self) -> Option<VariantRef<'a>> {
        match self.val {
            RVal::Obj(id) => match self.pool.get(id) {
                Obj::Variant {
                    type_name,
                    variant,
                    data,
                } => Some(VariantRef {
                    type_name: type_name.as_str(),
                    variant: variant.as_str(),
                    data: data.map(|v| ValueRef {
                        val: v,
                        pool: self.pool,
                    }),
                }),
                _ => None,
            },
            _ => None,
        }
    }

    pub fn as_some(&self) -> Option<ValueRef<'a>> {
        match self.val {
            RVal::Obj(id) => match self.pool.get(id) {
                Obj::Some(v) => Some(ValueRef {
                    val: *v,
                    pool: self.pool,
                }),
                _ => None,
            },
            _ => None,
        }
    }

    pub fn as_ok(&self) -> Option<ValueRef<'a>> {
        match self.val {
            RVal::Obj(id) => match self.pool.get(id) {
                Obj::Ok(v) => Some(ValueRef {
                    val: *v,
                    pool: self.pool,
                }),
                _ => None,
            },
            _ => None,
        }
    }

    pub fn as_err(&self) -> Option<ValueRef<'a>> {
        match self.val {
            RVal::Obj(id) => match self.pool.get(id) {
                Obj::Err(v) => Some(ValueRef {
                    val: *v,
                    pool: self.pool,
                }),
                _ => None,
            },
            _ => None,
        }
    }

    /// 所有型の [`crate::Value`] へ変換する（`std-value` feature 有効時）。
    #[cfg(feature = "std-value")]
    pub fn to_value(&self) -> crate::Value {
        self.pool.export(self.val)
    }
}

/// リストの要素を先頭からたどるゼロアロケーションイテレータ。
#[derive(Clone, Copy)]
pub struct ListIter<'a> {
    cur: RVal,
    pool: &'a Pool<'a>,
}

impl<'a> ListIter<'a> {
    #[inline]
    pub fn len(&self) -> usize {
        self.pool.list_len(self.cur)
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[inline]
    pub(crate) fn cur_rval(&self) -> RVal {
        self.cur
    }
}

impl<'a> Iterator for ListIter<'a> {
    type Item = ValueRef<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.cur {
            RVal::EmptyList => None,
            RVal::Obj(id) => match self.pool.get(id) {
                Obj::Cons { head, tail, .. } => {
                    self.cur = *tail;
                    Some(ValueRef {
                        val: *head,
                        pool: self.pool,
                    })
                }
                _ => None,
            },
            _ => None,
        }
    }
}

/// 配列・タプル要素の借用ビュー。
#[derive(Clone, Copy)]
pub struct SliceView<'a> {
    items: &'a [RVal],
    pool: &'a Pool<'a>,
}

impl<'a> SliceView<'a> {
    #[inline]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    #[inline]
    pub fn get(&self, index: usize) -> Option<ValueRef<'a>> {
        self.items.get(index).map(|&val| ValueRef {
            val,
            pool: self.pool,
        })
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = ValueRef<'a>> + 'a {
        let pool = self.pool;
        self.items.iter().map(move |&val| ValueRef { val, pool })
    }
}

/// 構造体の借用ビュー。
#[derive(Clone, Copy)]
pub struct StructRef<'a> {
    desc: StructDescRef,
    values: &'a [RVal],
    pool: &'a Pool<'a>,
}

impl<'a> StructRef<'a> {
    #[inline]
    pub fn name(&self) -> &'a str {
        self.pool.struct_desc_name(self.desc)
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn field(&self, key: &str) -> Option<ValueRef<'a>> {
        let idx = self.pool.struct_desc_find_field(self.desc, key)?;
        Some(ValueRef {
            val: self.values[idx],
            pool: self.pool,
        })
    }

    pub fn get(&self, index: usize) -> Option<(&'a str, ValueRef<'a>)> {
        let val = *self.values.get(index)?;
        let key = self.pool.struct_desc_field_name(self.desc, index);
        Some((
            key,
            ValueRef {
                val,
                pool: self.pool,
            },
        ))
    }

    pub fn fields(&self) -> impl ExactSizeIterator<Item = (&'a str, ValueRef<'a>)> + 'a {
        let this = *self;
        (0..this.values.len()).map(move |i| {
            (
                this.pool.struct_desc_field_name(this.desc, i),
                ValueRef {
                    val: this.values[i],
                    pool: this.pool,
                },
            )
        })
    }
}

/// バリアントの借用ビュー。
#[derive(Clone, Copy)]
pub struct VariantRef<'a> {
    type_name: &'a str,
    variant: &'a str,
    data: Option<ValueRef<'a>>,
}

impl<'a> VariantRef<'a> {
    #[inline]
    pub fn type_name(&self) -> &'a str {
        self.type_name
    }

    #[inline]
    pub fn variant(&self) -> &'a str {
        self.variant
    }

    #[inline]
    pub fn data(&self) -> Option<ValueRef<'a>> {
        self.data
    }
}

impl fmt::Display for ValueRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 深い入れ子でもネイティブスタックを溢れさせず、固定スタックで表示する。
        enum Frame<'a> {
            Val(RVal),
            Text(&'static str),
            Key(&'a str),
            ListTail(RVal),
        }
        let mut stack: [Option<Frame<'_>>; 64] = [const { None }; 64];
        let mut sp = 0usize;
        fn push<'a>(stack: &mut [Option<Frame<'a>>; 64], sp: &mut usize, item: Frame<'a>) -> bool {
            if *sp < stack.len() {
                stack[*sp] = Some(item);
                *sp += 1;
                true
            } else {
                false
            }
        }
        push(&mut stack, &mut sp, Frame::Val(self.val));
        while sp > 0 {
            sp -= 1;
            let Some(frame) = stack[sp].take() else {
                continue;
            };
            match frame {
                Frame::Text(t) => f.write_str(t)?,
                Frame::Key(k) => write!(f, "{k}: ")?,
                Frame::ListTail(mut tail) => {
                    while let RVal::Obj(id) = tail
                        && let Obj::Cons {
                            head, tail: next, ..
                        } = self.pool.get(id)
                    {
                        f.write_str(" ")?;
                        if !push(&mut stack, &mut sp, Frame::ListTail(*next))
                            || !push(&mut stack, &mut sp, Frame::Val(*head))
                        {
                            f.write_str("...")?;
                            break;
                        }
                        tail = RVal::EmptyList;
                    }
                }
                Frame::Val(v) => match v {
                    RVal::Void => f.write_str("void")?,
                    RVal::Int(n) => write!(f, "{n}")?,
                    RVal::Float(x) => {
                        if x.is_nan() {
                            f.write_str("nan")?;
                        } else if x.is_infinite() {
                            f.write_str(if x.is_sign_positive() { "inf" } else { "-inf" })?;
                        } else {
                            write!(f, "{x}")?;
                        }
                    }
                    RVal::Bool(b) => f.write_str(if b { "true" } else { "false" })?,
                    RVal::Char(c) => write!(f, "{c:?}")?,
                    RVal::None => f.write_str("none")?,
                    RVal::EmptyList => f.write_str("[]")?,
                    RVal::Builtin(b) => write!(f, "<builtin {}>", b.name())?,
                    RVal::Obj(id) => match self.pool.get(id) {
                        Obj::Str(s) => write!(f, "{:?}", s.as_str())?,
                        Obj::Ident(s) => write!(f, "'{}", s.as_str())?,
                        Obj::Cons { head, tail, .. } => {
                            f.write_char('[')?;
                            if !push(&mut stack, &mut sp, Frame::Text("]"))
                                || !push(&mut stack, &mut sp, Frame::ListTail(*tail))
                                || !push(&mut stack, &mut sp, Frame::Val(*head))
                            {
                                f.write_str("...]")?;
                            }
                        }
                        Obj::Array(xs) => {
                            f.write_str("#[")?;
                            let items = xs.as_slice();
                            if !push(&mut stack, &mut sp, Frame::Text("]")) {
                                f.write_str("...]")?;
                                continue;
                            }
                            for (i, &item) in items.iter().enumerate().rev() {
                                if !push(&mut stack, &mut sp, Frame::Val(item)) {
                                    f.write_str("...")?;
                                    break;
                                }
                                if i > 0 && !push(&mut stack, &mut sp, Frame::Text(" ")) {
                                    f.write_str("...")?;
                                    break;
                                }
                            }
                        }
                        Obj::Tuple(xs) => {
                            let items = xs.as_slice();
                            f.write_str(if items.is_empty() {
                                "(tuple"
                            } else {
                                "(tuple "
                            })?;
                            if !push(&mut stack, &mut sp, Frame::Text(")")) {
                                f.write_str("...)")?;
                                continue;
                            }
                            for (i, &item) in items.iter().enumerate().rev() {
                                if !push(&mut stack, &mut sp, Frame::Val(item)) {
                                    f.write_str("...")?;
                                    break;
                                }
                                if i > 0 && !push(&mut stack, &mut sp, Frame::Text(" ")) {
                                    f.write_str("...")?;
                                    break;
                                }
                            }
                        }
                        Obj::Struct { desc, values } => {
                            let name = self.pool.struct_desc_name(*desc);
                            write!(f, "{name} {{")?;
                            if values.is_empty() {
                                f.write_char('}')?;
                                continue;
                            }
                            f.write_char(' ')?;
                            if !push(&mut stack, &mut sp, Frame::Text(" }")) {
                                f.write_str("... }")?;
                                continue;
                            }
                            for i in (0..values.len()).rev() {
                                let k = self.pool.struct_desc_field_name(*desc, i);
                                let val = values.as_slice()[i];
                                if !push(&mut stack, &mut sp, Frame::Val(val))
                                    || !push(&mut stack, &mut sp, Frame::Key(k))
                                {
                                    f.write_str("...")?;
                                    break;
                                }
                                if i > 0 && !push(&mut stack, &mut sp, Frame::Text(" ")) {
                                    f.write_str("...")?;
                                    break;
                                }
                            }
                        }
                        Obj::Variant {
                            type_name,
                            variant,
                            data,
                        } => {
                            write!(f, "{}.{}", type_name.as_str(), variant.as_str())?;
                            if let Some(d) = *data {
                                f.write_char('(')?;
                                if !push(&mut stack, &mut sp, Frame::Text(")"))
                                    || !push(&mut stack, &mut sp, Frame::Val(d))
                                {
                                    f.write_str("...)")?;
                                }
                            }
                        }
                        Obj::Some(v) => {
                            f.write_str("some(")?;
                            if !push(&mut stack, &mut sp, Frame::Text(")"))
                                || !push(&mut stack, &mut sp, Frame::Val(*v))
                            {
                                f.write_str("...)")?;
                            }
                        }
                        Obj::Ok(v) => {
                            f.write_str("ok(")?;
                            if !push(&mut stack, &mut sp, Frame::Text(")"))
                                || !push(&mut stack, &mut sp, Frame::Val(*v))
                            {
                                f.write_str("...)")?;
                            }
                        }
                        Obj::Err(v) => {
                            f.write_str("err(")?;
                            if !push(&mut stack, &mut sp, Frame::Text(")"))
                                || !push(&mut stack, &mut sp, Frame::Val(*v))
                            {
                                f.write_str("...)")?;
                            }
                        }
                        Obj::Closure { proto, .. } => match proto.name {
                            Some(name) => write!(f, "<fn {}>", name.as_str())?,
                            None => f.write_str("<fn>")?,
                        },
                        Obj::Module { name, .. } => write!(f, "<module {}>", name.as_str())?,
                        Obj::StructDesc { name, .. } => {
                            write!(f, "<struct_desc {}>", name.as_str())?
                        }
                    },
                },
            }
        }
        Ok(())
    }
}

impl fmt::Debug for ValueRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ValueRef({self})")
    }
}
