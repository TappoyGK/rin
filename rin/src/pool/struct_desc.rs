//! 構造体記述子のインターン表と参照・診断補助。

use crate::error::{ErrRecorded, ErrorKind, Span};
use crate::mem::{AllocCore, PermArena, RawMap, RawStr, RawVec};

use super::{CHUNK_SLOTS, Obj, ObjStr, Pool, Slot, StructDescRef};

pub(super) struct PermStructDesc {
    pub(super) name: RawStr,
    pub(super) fields: RawVec<RawStr>,
}

pub(super) struct StructDescTable {
    pub(super) descs: RawVec<PermStructDesc>,
    by_hash: RawMap<u32, u32>,
}

impl StructDescTable {
    pub(super) const fn new() -> Self {
        Self {
            descs: RawVec::new(),
            by_hash: RawMap::new(),
        }
    }

    fn hash_desc(name: &str, sorted_fields: &[&str]) -> u64 {
        let mut h: u64 = 0xcbf29ce484222325;
        let mix = |h: &mut u64, bytes: &[u8]| {
            for &b in bytes {
                *h ^= b as u64;
                *h = h.wrapping_mul(0x100000001b3);
            }
            *h ^= 0xff;
            *h = h.wrapping_mul(0x100000001b3);
        };
        mix(&mut h, name.as_bytes());
        for &f in sorted_fields {
            mix(&mut h, f.as_bytes());
        }
        h
    }

    fn matches(&self, id: u32, name: &str, sorted_fields: &[&str]) -> bool {
        let desc = &self.descs.as_slice()[id as usize];
        if desc.name.as_str() != name || desc.fields.len() != sorted_fields.len() {
            return false;
        }
        desc.fields
            .as_slice()
            .iter()
            .zip(sorted_fields.iter())
            .all(|(a, b)| a.as_str() == *b)
    }

    fn find(&self, name: &str, sorted_fields: &[&str]) -> Option<u32> {
        let h = Self::hash_desc(name, sorted_fields);
        self.by_hash
            .get_by(h, |&id| self.matches(id, name, sorted_fields))
    }

    fn intern(
        &mut self,
        perm: &mut PermArena,
        core: &mut AllocCore<'_>,
        name: &str,
        sorted_fields: &[&str],
    ) -> Option<u32> {
        if let Some(id) = self.find(name, sorted_fields) {
            return Some(id);
        }
        if perm.is_frozen() {
            return None;
        }
        let h = Self::hash_desc(name, sorted_fields);
        let perm_name = RawStr::from_perm(perm, core, name)?;
        let mut perm_fields = RawVec::new();
        perm_fields.reserve_perm(perm, core, sorted_fields.len())?;
        for &f in sorted_fields {
            let fs = RawStr::from_perm(perm, core, f)?;
            perm_fields.push_perm(perm, core, fs)?;
        }
        let id = self.descs.len() as u32;
        self.descs.push_perm(
            perm,
            core,
            PermStructDesc {
                name: perm_name,
                fields: perm_fields,
            },
        )?;
        let descs_ptr = self.descs.as_slice().as_ptr();
        let descs_len = self.descs.len();
        self.by_hash.insert_perm(perm, core, h, id, id, |&a, &b| {
            // SAFETY: `descs` は直前に確定済み。
            let slice = unsafe { std::slice::from_raw_parts(descs_ptr, descs_len) };
            let da = &slice[a as usize];
            let db = &slice[b as usize];
            da.name.as_str() == db.name.as_str()
                && da.fields.len() == db.fields.len()
                && da
                    .fields
                    .as_slice()
                    .iter()
                    .zip(db.fields.as_slice().iter())
                    .all(|(x, y)| x.as_str() == y.as_str())
        })?;
        Some(id)
    }
}

impl Pool<'_> {
    /// 構造体記述子（フィールド名は辞書順ソート済み）をインターンまたは確保する。
    pub fn intern_struct_desc(
        &mut self,
        name: &str,
        sorted_fields: &[&str],
        span: Span,
    ) -> Result<StructDescRef, ErrRecorded> {
        if let Some(id) = self.struct_descs.find(name, sorted_fields) {
            return Ok(StructDescRef::Perm(id));
        }
        if !self.perm.is_frozen() {
            if let Some(id) =
                self.struct_descs
                    .intern(&mut self.perm, &mut self.core, name, sorted_fields)
            {
                return Ok(StructDescRef::Perm(id));
            }
            return Err(self.oom_limit_error(span));
        }
        // 凍結後の新規構造体はオブジェクト領域に参照カウント付き記述子として置く。
        let os_name = self.alloc_obj_str(name, span)?;
        let mut fields_slice =
            match self.alloc_obj_slice_uninit::<ObjStr>(sorted_fields.len(), span) {
                Ok(s) => s,
                Err(e) => {
                    // SAFETY: 直前に確保した `os_name`。
                    unsafe { self.obj_heap.free(&mut self.core, os_name.block()) };
                    return Err(e);
                }
            };
        let mut init_len = 0usize;
        for &f in sorted_fields {
            match self.alloc_obj_str(f, span) {
                Ok(fs) => {
                    fields_slice.as_mut_slice()[init_len] = fs;
                    init_len += 1;
                }
                Err(e) => {
                    for &prev in &fields_slice.as_slice()[..init_len] {
                        // SAFETY: 初期化済みの `ObjStr`。
                        unsafe { self.obj_heap.free(&mut self.core, prev.block()) };
                    }
                    // SAFETY: 直前に確保した領域。
                    unsafe {
                        self.obj_heap.free(&mut self.core, fields_slice.block());
                        self.obj_heap.free(&mut self.core, os_name.block());
                    }
                    return Err(e);
                }
            }
        }
        let desc_id = self.alloc_obj(
            Obj::StructDesc {
                name: os_name,
                fields: fields_slice,
            },
            span,
        )?;
        Ok(StructDescRef::Obj(desc_id))
    }

    pub fn struct_desc_name(&self, desc: StructDescRef) -> &str {
        match desc {
            StructDescRef::Perm(id) => self.struct_descs.descs.as_slice()[id as usize]
                .name
                .as_str(),
            StructDescRef::Obj(id) => match self.get(id) {
                Obj::StructDesc { name, .. } => name.as_str(),
                _ => unreachable!(),
            },
        }
    }

    #[cfg(feature = "std-value")]
    pub fn struct_desc_field_count(&self, desc: StructDescRef) -> usize {
        match desc {
            StructDescRef::Perm(id) => self.struct_descs.descs.as_slice()[id as usize].fields.len(),
            StructDescRef::Obj(id) => match self.get(id) {
                Obj::StructDesc { fields, .. } => fields.len(),
                _ => unreachable!(),
            },
        }
    }

    pub fn struct_desc_field_name(&self, desc: StructDescRef, idx: usize) -> &str {
        match desc {
            StructDescRef::Perm(id) => self.struct_descs.descs.as_slice()[id as usize]
                .fields
                .as_slice()[idx]
                .as_str(),
            StructDescRef::Obj(id) => match self.get(id) {
                Obj::StructDesc { fields, .. } => fields[idx].as_str(),
                _ => unreachable!(),
            },
        }
    }

    pub fn struct_desc_find_field(&self, desc: StructDescRef, field: &str) -> Option<usize> {
        match desc {
            StructDescRef::Perm(id) => self.struct_descs.descs.as_slice()[id as usize]
                .fields
                .as_slice()
                .binary_search_by(|f| f.as_str().cmp(field))
                .ok(),
            StructDescRef::Obj(id) => match self.get(id) {
                Obj::StructDesc { fields, .. } => fields
                    .as_slice()
                    .binary_search_by(|f| f.as_str().cmp(field))
                    .ok(),
                _ => unreachable!(),
            },
        }
    }

    pub fn struct_desc_retain(&mut self, desc: StructDescRef) {
        if let StructDescRef::Obj(id) = desc {
            self.retain(id);
        }
    }

    pub(crate) fn copy_struct_desc_name_to_work(&mut self, desc: StructDescRef) -> Option<RawStr> {
        let name = match desc {
            StructDescRef::Perm(id) => self.struct_descs.descs.as_slice()[id as usize]
                .name
                .as_str(),
            StructDescRef::Obj(id) => {
                let idx = id.0 as usize;
                let chunk_idx = idx / CHUNK_SLOTS;
                let in_chunk = idx % CHUNK_SLOTS;
                let len = if chunk_idx + 1 == self.chunks.len() {
                    self.last_chunk_len
                } else {
                    CHUNK_SLOTS
                };
                match &self.chunks.as_slice()[chunk_idx].as_slice(len)[in_chunk] {
                    Slot::Live {
                        obj: Obj::StructDesc { name, .. },
                        ..
                    } => name.as_str(),
                    _ => unreachable!(),
                }
            }
        };
        RawStr::from_work(&mut self.work, &mut self.core, name)
    }

    pub(crate) fn copy_struct_desc_field_to_work(
        &mut self,
        desc: StructDescRef,
        field_idx: usize,
    ) -> Option<RawStr> {
        let fname = match desc {
            StructDescRef::Perm(id) => self.struct_descs.descs.as_slice()[id as usize]
                .fields
                .as_slice()[field_idx]
                .as_str(),
            StructDescRef::Obj(id) => {
                let idx = id.0 as usize;
                let chunk_idx = idx / CHUNK_SLOTS;
                let in_chunk = idx % CHUNK_SLOTS;
                let len = if chunk_idx + 1 == self.chunks.len() {
                    self.last_chunk_len
                } else {
                    CHUNK_SLOTS
                };
                match &self.chunks.as_slice()[chunk_idx].as_slice(len)[in_chunk] {
                    Slot::Live {
                        obj: Obj::StructDesc { fields, .. },
                        ..
                    } => fields[field_idx].as_str(),
                    _ => unreachable!(),
                }
            }
        };
        RawStr::from_work(&mut self.work, &mut self.core, fname)
    }

    #[cold]
    #[inline(never)]
    pub fn err_struct_missing_field(
        &mut self,
        rule: &'static str,
        span: Span,
        desc: StructDescRef,
        field: &str,
    ) -> ErrRecorded {
        use std::fmt::Write;
        let out = self.core.error.begin_msg(rule, ErrorKind::Runtime, span);
        match desc {
            StructDescRef::Perm(id) => {
                let d = &self.struct_descs.descs.as_slice()[id as usize];
                let _ = write!(out, "{} has no field '{field}'", d.name.as_str());
                crate::diagnostic::write_available(
                    out,
                    "fields",
                    d.fields.as_slice().iter().map(RawStr::as_str),
                );
            }
            StructDescRef::Obj(id) => {
                let idx = id.0 as usize;
                let chunk_idx = idx / CHUNK_SLOTS;
                let in_chunk = idx % CHUNK_SLOTS;
                let len = if chunk_idx + 1 == self.chunks.len() {
                    self.last_chunk_len
                } else {
                    CHUNK_SLOTS
                };
                let slot = &self.chunks.as_slice()[chunk_idx].as_slice(len)[in_chunk];
                let Slot::Live {
                    obj: Obj::StructDesc { name, fields },
                    ..
                } = slot
                else {
                    unreachable!();
                };
                let _ = write!(out, "{} has no field '{field}'", name.as_str());
                crate::diagnostic::write_available(
                    out,
                    "fields",
                    fields.as_slice().iter().map(ObjStr::as_str),
                );
            }
        }
        ErrRecorded
    }
}
