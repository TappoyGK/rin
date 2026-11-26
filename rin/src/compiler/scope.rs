//! 束縛・キャプチャの解決と名前診断の記録（`WorkArena` + `CodeRegion` 完結）。
use super::*;

// 少数の束縛ではハッシュ表の確保を避ける。線形走査は常にこの定数以内に限る。
const INLINE_NAMES: usize = 8;
const REMOVED_LOCAL: u32 = u32::MAX;

pub(super) struct Scope {
    pub(super) module: Option<u32>,
    // AST の RawStr をそのまま保持し、診断に必要になった束縛だけノードの添字 + 1 を記録する。
    pub(super) locals: RawVec<(RawStr, u32, Option<NonZeroU32>, Option<NonZeroU32>)>,
    local_names: RawMap<RawStr, u32>,
    capture_names: RawMap<RawStr, u32>,
    pub(super) string_map: RawMap<RawStr, u32>,
    pub(super) outer_local_count: usize,
    pub(super) next_slot: u32,
    pub(super) max_slots: u32,
    pub(super) captures: RawVec<(RawStr, Capture)>,
    // 未発行の Tick が参照する範囲の開始位置。位置情報は最終格納先へ直接書く。
    pub(super) pending_tick_start: usize,
    pub(super) chunk: Chunk,
}

impl Scope {
    pub(super) fn new(module: Option<u32>) -> Self {
        Self {
            module,
            locals: RawVec::new(),
            local_names: RawMap::new(),
            capture_names: RawMap::new(),
            string_map: RawMap::new(),
            outer_local_count: 0,
            next_slot: 0,
            max_slots: 0,
            captures: RawVec::new(),
            pending_tick_start: 0,
            chunk: Chunk::default(),
        }
    }

    pub(super) fn alloc_slot(
        &mut self,
        pool: &mut Pool<'_>,
        name: RawStr,
        span: Span,
    ) -> Result<u32, ErrRecorded> {
        let slot = self.next_slot;
        let Some(next_slot) = self.next_slot.checked_add(1) else {
            return Err(pool.core.err_str(
                "compile.capacity",
                ErrorKind::Parse,
                span,
                "too many local variables",
            ));
        };
        self.next_slot = next_slot;
        if self.next_slot > self.max_slots {
            self.max_slots = self.next_slot;
        }
        let index = self.locals.len() as u32;
        let previous = if !self.local_names.is_empty() {
            let hash = fnv1a_bytes(name.as_str().as_bytes());
            let Some(prev) = self.local_names.insert_work(
                &mut pool.work,
                &mut pool.core,
                hash,
                name,
                index,
                |a, b| a.as_str() == b.as_str(),
            ) else {
                return Err(pool.core.err_oom(span));
            };
            prev.filter(|&p| p != REMOVED_LOCAL)
        } else {
            self.locals
                .as_slice()
                .iter()
                .rposition(|(n, _, _, _)| n.as_str() == name.as_str())
                .map(|i| i as u32)
        };
        let previous = previous.and_then(|i| NonZeroU32::new(i + 1));
        if self
            .locals
            .push_work(&mut pool.work, &mut pool.core, (name, slot, None, previous))
            .is_none()
        {
            return Err(pool.core.err_oom(span));
        }
        if self.local_names.is_empty() && self.locals.len() > INLINE_NAMES {
            for (i, &(n, _, _, _)) in self.locals.as_slice().iter().enumerate() {
                let hash = fnv1a_bytes(n.as_str().as_bytes());
                if self
                    .local_names
                    .insert_work(
                        &mut pool.work,
                        &mut pool.core,
                        hash,
                        n,
                        i as u32,
                        |a, b| a.as_str() == b.as_str(),
                    )
                    .is_none()
                {
                    return Err(pool.core.err_oom(span));
                }
            }
        }
        Ok(slot)
    }

    pub(super) fn truncate_locals(&mut self, pool: &mut Pool<'_>, len: usize) {
        while self.locals.len() > len {
            let (name, _, _, previous) = self.locals.pop().unwrap();
            if !self.local_names.is_empty() {
                let hash = fnv1a_bytes(name.as_str().as_bytes());
                let val = previous.map_or(REMOVED_LOCAL, |p| p.get() - 1);
                // 既に存在するキーの上書きなので確保は起きない。
                let _ = self.local_names.insert_work(
                    &mut pool.work,
                    &mut pool.core,
                    hash,
                    name,
                    val,
                    |a, b| a.as_str() == b.as_str(),
                );
            }
        }
    }

    fn find_local(&self, name: &str) -> Option<u32> {
        if !self.local_names.is_empty() {
            let hash = fnv1a_bytes(name.as_bytes());
            let idx = self.local_names.get_by(hash, |k| k.as_str() == name)?;
            if idx == REMOVED_LOCAL {
                None
            } else {
                Some(self.locals.as_slice()[idx as usize].1)
            }
        } else {
            self.locals
                .as_slice()
                .iter()
                .rev()
                .find(|(n, _, _, _)| n.as_str() == name)
                .map(|(_, slot, _, _)| *slot)
        }
    }

    fn find_or_add_capture(
        &mut self,
        pool: &mut Pool<'_>,
        name: RawStr,
        cap: Capture,
        span: Span,
    ) -> Result<u32, ErrRecorded> {
        let name_str = name.as_str();
        let found = if !self.capture_names.is_empty() {
            let hash = fnv1a_bytes(name_str.as_bytes());
            self.capture_names.get_by(hash, |k| k.as_str() == name_str)
        } else {
            self.captures
                .as_slice()
                .iter()
                .position(|(n, _)| n.as_str() == name_str)
                .map(|i| i as u32)
        };
        if let Some(index) = found {
            return Ok(index);
        }
        let Ok(idx) = u32::try_from(self.captures.len()) else {
            return Err(pool.core.err_str(
                "compile.capacity",
                ErrorKind::Parse,
                span,
                "too many captured variables",
            ));
        };
        if self
            .captures
            .push_work(&mut pool.work, &mut pool.core, (name, cap))
            .is_none()
        {
            return Err(pool.core.err_oom(span));
        }
        if !self.capture_names.is_empty() {
            let hash = fnv1a_bytes(name_str.as_bytes());
            if self
                .capture_names
                .insert_work(
                    &mut pool.work,
                    &mut pool.core,
                    hash,
                    name,
                    idx,
                    |a, b| a.as_str() == b.as_str(),
                )
                .is_none()
            {
                return Err(pool.core.err_oom(span));
            }
        } else if self.captures.len() > INLINE_NAMES {
            for (i, &(n, _)) in self.captures.as_slice().iter().enumerate() {
                let hash = fnv1a_bytes(n.as_str().as_bytes());
                if self
                    .capture_names
                    .insert_work(
                        &mut pool.work,
                        &mut pool.core,
                        hash,
                        n,
                        i as u32,
                        |a, b| a.as_str() == b.as_str(),
                    )
                    .is_none()
                {
                    return Err(pool.core.err_oom(span));
                }
            }
        }
        Ok(idx)
    }
}

impl<'a, 'p> Compiler<'a, 'p> {
    pub(super) fn new_scope(&self) -> Scope {
        let parent = self.scopes.last().unwrap();
        let mut inner = Scope::new(self.module);
        inner.outer_local_count = parent.outer_local_count + parent.locals.len();
        inner
    }

    // 最後に記録したノードまで戻り、増えた束縛だけを追加する。
    // 兄弟の関数・クロージャは同じ親ノードを参照する。
    fn visible_name_head(&mut self, span: Span) -> Result<Option<usize>, Failed> {
        let mut pending_from = None;
        let mut head = None;
        'scopes: for (scope_index, scope) in self.scopes.as_slice().iter().enumerate().rev() {
            for (local_index, (_, _, saved, _)) in scope.locals.as_slice().iter().enumerate().rev() {
                if let Some(saved) = saved {
                    head = Some(saved.get() as usize - 1);
                    break 'scopes;
                }
                pending_from = Some((scope_index, local_index));
            }
        }
        if let Some((first_scope, first_local)) = pending_from {
            let mut table_ptr = self.name_table.expect("name table initialized");
            let region = self.region;
            for (index, scope) in self
                .scopes
                .as_mut_slice()
                .iter_mut()
                .enumerate()
                .skip(first_scope)
            {
                let start = if index == first_scope { first_local } else { 0 };
                for (name, _, saved, _) in &mut scope.locals.as_mut_slice()[start..] {
                    debug_assert!(saved.is_none());
                    // SAFETY: コンパイル中は `Compiler` が `table_ptr` を排他的に所有している。
                    let table = unsafe { table_ptr.as_mut() };
                    let node_index = table.nodes.len();
                    let text_start = table.text.len();
                    if self
                        .pool
                        .extend_code_slice(region, &mut table.text, name.as_str().as_bytes())
                        .is_none()
                    {
                        return Err(self.fail_oom(span));
                    }
                    let text_end = table.text.len();
                    if self
                        .pool
                        .push_code(
                            region,
                            &mut table.nodes,
                            NameNode {
                                start: text_start,
                                end: text_end,
                                parent: head,
                            },
                        )
                        .is_none()
                    {
                        return Err(self.fail_oom(span));
                    }
                    *saved = NonZeroU32::new(node_index as u32 + 1);
                    head = Some(node_index);
                }
            }
        }
        Ok(head)
    }

    // 大域の束縛は上書きされても消えない。既存の値があれば未定義名にならない。
    // 再読込で消えるモジュールの束縛は、この判定に含めない。
    #[cold]
    pub(super) fn record_name_site(&mut self, id: u32, ip: usize, span: Span) -> Result<(), Failed> {
        let scope = self.scopes.last().unwrap();
        // ローカルが見えない場所では文字数も調べず、表の準備にも入らない。
        if scope.outer_local_count == 0 && scope.locals.is_empty() {
            return Ok(());
        }
        let Some(sym_name) = self.syms.name(id) else {
            return Ok(());
        };
        if !needs_name_candidates(sym_name) {
            return Ok(());
        }
        let mut table_ptr = match self.name_table {
            Some(ptr) => ptr,
            None => {
                let Some(ptr) = self.pool.alloc_code_val(self.region, NameTable::default()) else {
                    return Err(self.fail_oom(span));
                };
                self.name_table = Some(ptr);
                ptr
            }
        };
        if let Some(head) = self.visible_name_head(span)? {
            let previous = self.chunk().names.map(|names| names.last_site);
            // SAFETY: コンパイル中は `Compiler` が `table_ptr` を排他的に所有している。
            let table = unsafe { table_ptr.as_mut() };
            let site = table.sites.len();
            if self
                .pool
                .push_code(self.region, &mut table.sites, NameSite { ip, head, previous })
                .is_none()
            {
                return Err(self.fail_oom(span));
            }
            if let Some(names) = &mut self.chunk().names {
                names.last_site = site;
            } else {
                self.chunk().names = Some(NameInfo {
                    table: table_ptr,
                    last_site: site,
                });
            }
        }
        Ok(())
    }

    pub(super) fn resolve_var(&mut self, name: RawStr, span: Span) -> Result<Op, Failed> {
        let top = self.scopes.len() - 1;
        if let Some(slot) = self.scopes[top].find_local(name.as_str()) {
            return Ok(Op::GetLocal(slot));
        }
        if top > 0
            && let Some(cap_idx) = self.resolve_capture(top, name, span)?
        {
            return Ok(Op::GetCapture(cap_idx));
        }
        let id = self.intern_sym(name.as_str(), span)?;
        Ok(Op::GetGlobal(id))
    }

    fn resolve_capture(
        &mut self,
        scope_idx: usize,
        name: RawStr,
        span: Span,
    ) -> Result<Option<u32>, Failed> {
        // 内側から外側へ定義元のスコープを非再帰で探し、戻りながら各中間スコープへキャプチャを伝播する。
        let mut owner = None;
        let mut cur = scope_idx;
        while cur > 0 {
            let parent = cur - 1;
            if let Some(slot) = self.scopes[parent].find_local(name.as_str()) {
                owner = Some((cur, slot));
                break;
            }
            cur = parent;
        }
        let Some((first_capture_scope, slot)) = owner else {
            return Ok(None);
        };
        let mut cap_idx = self.scopes[first_capture_scope].find_or_add_capture(
            self.pool,
            name,
            Capture::Local(slot),
            span,
        )?;
        for s in (first_capture_scope + 1)..=scope_idx {
            cap_idx = self.scopes[s].find_or_add_capture(
                self.pool,
                name,
                Capture::Upvalue(cap_idx),
                span,
            )?;
        }
        Ok(Some(cap_idx))
    }
}
