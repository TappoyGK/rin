//! 永続領域 (`PermArena`) に格納する確定済み型表現と型環境 (`TypeEnv`)。
//!
//! 読み込み段階で `defschema`・モジュール公開型・`def` / `defn` の確定済みシグネチャを
//! `PermArena` に保存し、`Builder::freeze()` 後は読み取り専用として扱う。

use std::fmt;

use crate::error::{ErrRecorded, ErrorKind, Span};
use crate::mem::{AllocCore, PermArena, RawMap, RawStr, RawVec, fnv1a_bytes};

/// 永続領域上の確定済み型 ID。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct TypeId(pub(crate) u32);

pub(crate) const TY_VOID: TypeId = TypeId(0);
pub(crate) const TY_I64: TypeId = TypeId(1);
pub(crate) const TY_F64: TypeId = TypeId(2);
pub(crate) const TY_BOOL: TypeId = TypeId(3);
pub(crate) const TY_CHAR: TypeId = TypeId(4);
pub(crate) const TY_TEXT: TypeId = TypeId(5);
pub(crate) const TY_IDENT: TypeId = TypeId(6);
pub(crate) const TY_DATUM: TypeId = TypeId(7);
pub(crate) const PRIMITIVE_TYPE_COUNT: usize = 8;

/// 永続領域上の固有スキーマ型 ID（`defschema` ごとに一意）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct SchemaId(pub(crate) u32);

/// 永続領域上の型ノード。
#[derive(Debug, Clone, Copy)]
pub(crate) enum PermTypeNode {
    Void,
    I64,
    F64,
    Bool,
    Char,
    Text,
    Ident,
    Datum,
    List(TypeId),
    Array(TypeId),
    Option(TypeId),
    Result { ok: TypeId, err: TypeId },
    Tuple(RawVec<TypeId>),
    Fn { params: RawVec<TypeId>, ret: TypeId },
    Schema(SchemaId),
}

/// 永続領域上のバリアント枝定義。
#[derive(Debug, Clone, Copy)]
pub(crate) struct VariantCase {
    pub(crate) name: RawStr,
    pub(crate) payload: Option<TypeId>,
}

/// 永続領域上のスキーマ本体。
#[derive(Debug, Clone, Copy)]
pub(crate) enum SchemaKind {
    Struct {
        open: bool,
        type_name: RawStr,
        fields: RawVec<(RawStr, TypeId)>,
    },
    Variant {
        type_name: RawStr,
        cases: RawVec<VariantCase>,
    },
}

/// 永続領域上のスキーマ定義。
#[derive(Debug, Clone, Copy)]
pub(crate) struct SchemaDef {
    pub(crate) name: RawStr,
    pub(crate) module: Option<RawStr>,
    pub(crate) kind: SchemaKind,
}

/// 永続領域上の関数シグネチャ（STEP 4 のシグネチャ取得 API でも共用）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct FnSig {
    pub(crate) params: RawVec<(RawStr, TypeId)>,
    pub(crate) ret: TypeId,
}

/// 永続領域上の値束縛の型情報。
#[derive(Debug, Clone, Copy)]
pub(crate) struct ValBinding {
    pub(crate) ty: TypeId,
    pub(crate) fn_sig: Option<FnSig>,
}

/// 永続領域上のモジュール公開型・値環境。
#[derive(Clone, Copy)]
pub(crate) struct ModuleTypeEnv {
    pub(crate) name: RawStr,
    pub(crate) exports_types: RawMap<RawStr, SchemaId>,
    pub(crate) exports_vals: RawMap<RawStr, ValBinding>,
    pub(crate) export_val_names: RawVec<RawStr>,
}

/// 失敗時のゼロ確保ロールバック用スナップショット。
#[derive(Clone, Copy)]
pub(crate) struct TypeEnvSnapshot {
    root_type_slots_len: usize,
    root_val_slots_len: usize,
    module_slots_len: usize,
    root_import_slots_len: usize,
}

/// 永続領域 (`PermArena`) に保持する型環境。
pub(crate) struct TypeEnv {
    types: RawVec<PermTypeNode>,
    schemas: RawVec<SchemaDef>,
    root_types: RawMap<RawStr, u32>,
    root_type_slots: RawVec<Option<SchemaId>>,
    root_vals: RawMap<RawStr, u32>,
    root_val_slots: RawVec<Option<ValBinding>>,
    root_val_names: RawVec<RawStr>,
    modules: RawMap<RawStr, u32>,
    module_slots: RawVec<Option<ModuleTypeEnv>>,
    root_imports: RawMap<RawStr, u32>,
    root_import_slots: RawVec<Option<u32>>,
}

impl TypeEnv {
    pub(crate) const fn new() -> Self {
        Self {
            types: RawVec::new(),
            schemas: RawVec::new(),
            root_types: RawMap::new(),
            root_type_slots: RawVec::new(),
            root_vals: RawMap::new(),
            root_val_slots: RawVec::new(),
            root_val_names: RawVec::new(),
            modules: RawMap::new(),
            module_slots: RawVec::new(),
            root_imports: RawMap::new(),
            root_import_slots: RawVec::new(),
        }
    }

    /// 組み込み 8 型ノード（ID `0..8`）を初期化する。
    pub(crate) fn ensure_primitives(
        &mut self,
        perm: &mut PermArena,
        core: &mut AllocCore<'_>,
        span: Span,
    ) -> Result<(), ErrRecorded> {
        if self.types.len() >= PRIMITIVE_TYPE_COUNT {
            return Ok(());
        }
        let prims = [
            PermTypeNode::Void,
            PermTypeNode::I64,
            PermTypeNode::F64,
            PermTypeNode::Bool,
            PermTypeNode::Char,
            PermTypeNode::Text,
            PermTypeNode::Ident,
            PermTypeNode::Datum,
        ];
        for node in prims {
            if self.types.push_perm(perm, core, node).is_none() {
                return Err(core.err_str(
                    "eval.limits",
                    ErrorKind::TooMuchMemory,
                    span,
                    "memory limit exceeded",
                ));
            }
        }
        Ok(())
    }

    #[inline]
    pub(crate) fn get_type(&self, id: TypeId) -> PermTypeNode {
        let idx = id.0 as usize;
        if idx < self.types.len() {
            self.types.as_slice()[idx]
        } else {
            match id {
                TY_VOID => PermTypeNode::Void,
                TY_I64 => PermTypeNode::I64,
                TY_F64 => PermTypeNode::F64,
                TY_BOOL => PermTypeNode::Bool,
                TY_CHAR => PermTypeNode::Char,
                TY_TEXT => PermTypeNode::Text,
                TY_IDENT => PermTypeNode::Ident,
                TY_DATUM => PermTypeNode::Datum,
                _ => unreachable!("invalid TypeId"),
            }
        }
    }

    pub(crate) fn alloc_type(
        &mut self,
        perm: &mut PermArena,
        core: &mut AllocCore<'_>,
        node: PermTypeNode,
        span: Span,
    ) -> Result<TypeId, ErrRecorded> {
        match node {
            PermTypeNode::Void => return Ok(TY_VOID),
            PermTypeNode::I64 => return Ok(TY_I64),
            PermTypeNode::F64 => return Ok(TY_F64),
            PermTypeNode::Bool => return Ok(TY_BOOL),
            PermTypeNode::Char => return Ok(TY_CHAR),
            PermTypeNode::Text => return Ok(TY_TEXT),
            PermTypeNode::Ident => return Ok(TY_IDENT),
            PermTypeNode::Datum => return Ok(TY_DATUM),
            _ => {}
        }
        self.ensure_primitives(perm, core, span)?;
        let id = TypeId(self.types.len() as u32);
        if self.types.push_perm(perm, core, node).is_none() {
            return Err(core.err_str(
                "eval.limits",
                ErrorKind::TooMuchMemory,
                span,
                "memory limit exceeded",
            ));
        }
        Ok(id)
    }

    pub(crate) fn alloc_schema(
        &mut self,
        perm: &mut PermArena,
        core: &mut AllocCore<'_>,
        def: SchemaDef,
        span: Span,
    ) -> Result<SchemaId, ErrRecorded> {
        let id = SchemaId(self.schemas.len() as u32);
        if self.schemas.push_perm(perm, core, def).is_none() {
            return Err(core.err_str(
                "eval.limits",
                ErrorKind::TooMuchMemory,
                span,
                "memory limit exceeded",
            ));
        }
        Ok(id)
    }

    #[inline]
    pub(crate) fn get_schema(&self, id: SchemaId) -> &SchemaDef {
        &self.schemas.as_slice()[id.0 as usize]
    }

    #[inline]
    pub(crate) fn get_schema_mut(&mut self, id: SchemaId) -> &mut SchemaDef {
        &mut self.schemas.as_mut_slice()[id.0 as usize]
    }

    pub(crate) fn lookup_root_type(&self, name: &str) -> Option<SchemaId> {
        let h = fnv1a_bytes(name.as_bytes());
        let slot_idx = self.root_types.get_by(h, |k| k.as_str() == name)?;
        self.root_type_slots.as_slice()[slot_idx as usize]
    }

    pub(crate) fn define_root_type(
        &mut self,
        perm: &mut PermArena,
        core: &mut AllocCore<'_>,
        name: &str,
        schema_id: SchemaId,
        span: Span,
    ) -> Result<(), ErrRecorded> {
        let h = fnv1a_bytes(name.as_bytes());
        if let Some(slot_idx) = self.root_types.get_by(h, |k| k.as_str() == name) {
            self.root_type_slots.as_mut_slice()[slot_idx as usize] = Some(schema_id);
            return Ok(());
        }
        let Some(raw_name) = RawStr::from_perm(perm, core, name) else {
            return Err(core.err_str(
                "eval.limits",
                ErrorKind::TooMuchMemory,
                span,
                "memory limit exceeded",
            ));
        };
        let slot_idx = self.root_type_slots.len() as u32;
        if self
            .root_type_slots
            .push_perm(perm, core, Some(schema_id))
            .is_none()
        {
            return Err(core.err_str(
                "eval.limits",
                ErrorKind::TooMuchMemory,
                span,
                "memory limit exceeded",
            ));
        }
        if self
            .root_types
            .insert_perm(perm, core, h, raw_name, slot_idx, |a, b| {
                a.as_str() == b.as_str()
            })
            .is_none()
        {
            return Err(core.err_str(
                "eval.limits",
                ErrorKind::TooMuchMemory,
                span,
                "memory limit exceeded",
            ));
        }
        Ok(())
    }

    pub(crate) fn lookup_root_val(&self, name: &str) -> Option<ValBinding> {
        let h = fnv1a_bytes(name.as_bytes());
        let slot_idx = self.root_vals.get_by(h, |k| k.as_str() == name)?;
        self.root_val_slots.as_slice()[slot_idx as usize]
    }

    pub(crate) fn define_root_val(
        &mut self,
        perm: &mut PermArena,
        core: &mut AllocCore<'_>,
        name: &str,
        binding: ValBinding,
        span: Span,
    ) -> Result<(), ErrRecorded> {
        let h = fnv1a_bytes(name.as_bytes());
        if let Some(slot_idx) = self.root_vals.get_by(h, |k| k.as_str() == name) {
            self.root_val_slots.as_mut_slice()[slot_idx as usize] = Some(binding);
            return Ok(());
        }
        let Some(raw_name) = RawStr::from_perm(perm, core, name) else {
            return Err(core.err_str(
                "eval.limits",
                ErrorKind::TooMuchMemory,
                span,
                "memory limit exceeded",
            ));
        };
        let slot_idx = self.root_val_slots.len() as u32;
        if self
            .root_val_slots
            .push_perm(perm, core, Some(binding))
            .is_none()
            || self
                .root_val_names
                .push_perm(perm, core, raw_name)
                .is_none()
        {
            return Err(core.err_str(
                "eval.limits",
                ErrorKind::TooMuchMemory,
                span,
                "memory limit exceeded",
            ));
        }
        if self
            .root_vals
            .insert_perm(perm, core, h, raw_name, slot_idx, |a, b| {
                a.as_str() == b.as_str()
            })
            .is_none()
        {
            return Err(core.err_str(
                "eval.limits",
                ErrorKind::TooMuchMemory,
                span,
                "memory limit exceeded",
            ));
        }
        Ok(())
    }

    pub(crate) fn lookup_module(&self, name: &str) -> Option<(u32, &ModuleTypeEnv)> {
        let h = fnv1a_bytes(name.as_bytes());
        let slot_idx = self.modules.get_by(h, |k| k.as_str() == name)?;
        let env = self.module_slots.as_slice()[slot_idx as usize].as_ref()?;
        Some((slot_idx, env))
    }

    pub(crate) fn get_module(&self, slot_idx: u32) -> Option<&ModuleTypeEnv> {
        self.module_slots
            .as_slice()
            .get(slot_idx as usize)?
            .as_ref()
    }

    pub(crate) fn define_module(
        &mut self,
        perm: &mut PermArena,
        core: &mut AllocCore<'_>,
        env: ModuleTypeEnv,
        span: Span,
    ) -> Result<u32, ErrRecorded> {
        let name = env.name.as_str();
        let h = fnv1a_bytes(name.as_bytes());
        if let Some(slot_idx) = self.modules.get_by(h, |k| k.as_str() == name) {
            self.module_slots.as_mut_slice()[slot_idx as usize] = Some(env);
            return Ok(slot_idx);
        }
        let slot_idx = self.module_slots.len() as u32;
        if self.module_slots.push_perm(perm, core, Some(env)).is_none() {
            return Err(core.err_str(
                "eval.limits",
                ErrorKind::TooMuchMemory,
                span,
                "memory limit exceeded",
            ));
        }
        if self
            .modules
            .insert_perm(perm, core, h, env.name, slot_idx, |a, b| {
                a.as_str() == b.as_str()
            })
            .is_none()
        {
            return Err(core.err_str(
                "eval.limits",
                ErrorKind::TooMuchMemory,
                span,
                "memory limit exceeded",
            ));
        }
        Ok(slot_idx)
    }

    pub(crate) fn lookup_root_import(&self, name: &str) -> Option<u32> {
        let h = fnv1a_bytes(name.as_bytes());
        let slot_idx = self.root_imports.get_by(h, |k| k.as_str() == name)?;
        self.root_import_slots.as_slice()[slot_idx as usize]
    }

    pub(crate) fn define_root_import(
        &mut self,
        perm: &mut PermArena,
        core: &mut AllocCore<'_>,
        name: &str,
        mod_slot: u32,
        span: Span,
    ) -> Result<(), ErrRecorded> {
        let h = fnv1a_bytes(name.as_bytes());
        if let Some(slot_idx) = self.root_imports.get_by(h, |k| k.as_str() == name) {
            self.root_import_slots.as_mut_slice()[slot_idx as usize] = Some(mod_slot);
            return Ok(());
        }
        let Some(raw_name) = RawStr::from_perm(perm, core, name) else {
            return Err(core.err_str(
                "eval.limits",
                ErrorKind::TooMuchMemory,
                span,
                "memory limit exceeded",
            ));
        };
        let slot_idx = self.root_import_slots.len() as u32;
        if self
            .root_import_slots
            .push_perm(perm, core, Some(mod_slot))
            .is_none()
            || self
                .root_imports
                .insert_perm(perm, core, h, raw_name, slot_idx, |a, b| {
                    a.as_str() == b.as_str()
                })
                .is_none()
        {
            return Err(core.err_str(
                "eval.limits",
                ErrorKind::TooMuchMemory,
                span,
                "memory limit exceeded",
            ));
        }
        Ok(())
    }

    pub(crate) fn iter_root_val_names(&self) -> impl Iterator<Item = &str> + '_ {
        self.root_val_slots
            .as_slice()
            .iter()
            .enumerate()
            .filter_map(|(i, slot)| slot.map(|_| self.root_val_names.as_slice()[i].as_str()))
    }

    pub(crate) fn iter_module_names(&self) -> impl Iterator<Item = &str> + '_ {
        self.module_slots
            .as_slice()
            .iter()
            .filter_map(|slot| slot.as_ref().map(|m| m.name.as_str()))
    }

    /// 失敗時の巻き戻し用スナップショットを取得する。
    pub(crate) fn snapshot(&self) -> TypeEnvSnapshot {
        TypeEnvSnapshot {
            root_type_slots_len: self.root_type_slots.len(),
            root_val_slots_len: self.root_val_slots.len(),
            module_slots_len: self.module_slots.len(),
            root_import_slots_len: self.root_import_slots.len(),
        }
    }

    /// 追加確保なしで指定スナップショット以降の新規定義を取り消す。
    pub(crate) fn rollback(&mut self, snap: TypeEnvSnapshot) {
        for slot in &mut self.root_type_slots.as_mut_slice()[snap.root_type_slots_len..] {
            *slot = None;
        }
        for slot in &mut self.root_val_slots.as_mut_slice()[snap.root_val_slots_len..] {
            *slot = None;
        }
        for slot in &mut self.module_slots.as_mut_slice()[snap.module_slots_len..] {
            *slot = None;
        }
        for slot in &mut self.root_import_slots.as_mut_slice()[snap.root_import_slots_len..] {
            *slot = None;
        }
    }

    /// 型を人間可読な文字列表現へ整形する（循環ガード付き・ヒープ確保なし）。
    pub(crate) fn fmt_type(&self, id: TypeId, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.fmt_type_depth(id, 0, f)
    }

    fn fmt_type_depth(&self, id: TypeId, depth: usize, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if depth > 32 {
            return f.write_str("...");
        }
        match self.get_type(id) {
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
                self.fmt_type_depth(inner, depth + 1, f)?;
                f.write_str(")")
            }
            PermTypeNode::Array(inner) => {
                f.write_str("(array ")?;
                self.fmt_type_depth(inner, depth + 1, f)?;
                f.write_str(")")
            }
            PermTypeNode::Option(inner) => {
                f.write_str("(option ")?;
                self.fmt_type_depth(inner, depth + 1, f)?;
                f.write_str(")")
            }
            PermTypeNode::Result { ok, err } => {
                f.write_str("(result ")?;
                self.fmt_type_depth(ok, depth + 1, f)?;
                f.write_str(" ")?;
                self.fmt_type_depth(err, depth + 1, f)?;
                f.write_str(")")
            }
            PermTypeNode::Tuple(items) => {
                f.write_str("(tuple")?;
                for &item in items.as_slice() {
                    f.write_str(" ")?;
                    self.fmt_type_depth(item, depth + 1, f)?;
                }
                f.write_str(")")
            }
            PermTypeNode::Fn { params, ret } => {
                f.write_str("(fn (")?;
                for (i, &p) in params.as_slice().iter().enumerate() {
                    if i > 0 {
                        f.write_str(" ")?;
                    }
                    self.fmt_type_depth(p, depth + 1, f)?;
                }
                f.write_str(") ")?;
                self.fmt_type_depth(ret, depth + 1, f)?;
                f.write_str(")")
            }
            PermTypeNode::Schema(sid) => {
                let s = self.get_schema(sid);
                if let Some(m) = s.module {
                    write!(f, "{}.{}", m.as_str(), s.name.as_str())
                } else {
                    f.write_str(s.name.as_str())
                }
            }
        }
    }
}

/// 組み込み型名かどうか（`defschema` での上書き禁止対象）。
pub(crate) fn is_builtin_type_name(name: &str) -> bool {
    matches!(
        name,
        "void"
            | "i64"
            | "f64"
            | "bool"
            | "char"
            | "text"
            | "ident"
            | "datum"
            | "list"
            | "array"
            | "option"
            | "result"
            | "tuple"
            | "fn"
            | "any"
            | "struct"
            | "open_struct"
            | "variant"
    )
}
