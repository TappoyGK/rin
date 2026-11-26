//! 束縛・キャプチャの解決と名前診断の記録。
use super::*;
use std::collections::BTreeMap;
use std::num::NonZeroU32;

// 少数の束縛では確保を避ける。線形走査は常にこの定数以内に限る。
const INLINE_NAMES: usize = 8;

pub(super) struct Scope<'a> {
    pub(super) module: Option<u32>,
    // AST の名前を借用し、診断に必要になった束縛だけノードの添字 + 1 を記録する。
    // NonZero により未記録状態も 1 ワードに収める。
    pub(super) locals: Vec<(&'a str, u32, Option<NonZeroUsize>, Option<NonZeroU32>)>,
    // Scope自体を小さく保ち、小さい関数のコンパイルで表の管理領域を持ち回らない。
    // Boxの確保は固定個数を超えたスコープだけで行う。
    #[allow(clippy::box_collection)]
    local_names: Option<Box<BTreeMap<&'a str, usize>>>,
    #[allow(clippy::box_collection)]
    capture_names: Option<Box<BTreeMap<Rc<str>, u32>>>,
    pub(super) outer_local_count: usize,
    pub(super) next_slot: u32,
    pub(super) max_slots: u32,
    pub(super) captures: Vec<(Rc<str>, Capture)>,
    // 未発行の Tick が参照する範囲の開始位置。位置情報は最終格納先へ直接書く。
    pub(super) pending_tick_start: usize,
    pub(super) chunk: Chunk,
}

impl<'a> Scope<'a> {
    pub(super) fn new(module: Option<u32>) -> Self {
        Self {
            module,
            locals: Vec::new(),
            local_names: None,
            capture_names: None,
            outer_local_count: 0,
            next_slot: 0,
            max_slots: 0,
            captures: Vec::new(),
            pending_tick_start: 0,
            chunk: Chunk::default(),
        }
    }

    pub(super) fn alloc_slot(&mut self, name: &'a str, span: Span) -> Result<u32, Error> {
        let slot = self.next_slot;
        self.next_slot = self
            .next_slot
            .checked_add(1)
            .ok_or_else(|| Error::parse("compile.capacity", span, "too many local variables"))?;
        if self.next_slot > self.max_slots {
            self.max_slots = self.next_slot;
        }
        let index = self.locals.len();
        let previous = if let Some(names) = &mut self.local_names {
            names.insert(name, index)
        } else {
            self.locals.iter().rposition(|(n, _, _, _)| *n == name)
        };
        // 局所スロット数はu32以内。添字+1を使い、未記録を含め4バイトに収める。
        let previous = previous.and_then(|i| NonZeroU32::new(i as u32 + 1));
        self.locals.push((name, slot, None, previous));
        if self.local_names.is_none() && self.locals.len() > INLINE_NAMES {
            let mut names = BTreeMap::new();
            for (i, (name, _, _, _)) in self.locals.iter().enumerate() {
                names.insert(*name, i);
            }
            self.local_names = Some(Box::new(names));
        }
        Ok(slot)
    }

    pub(super) fn truncate_locals(&mut self, len: usize) {
        while self.locals.len() > len {
            let (name, _, _, previous) = self.locals.pop().unwrap();
            if let Some(names) = &mut self.local_names {
                if let Some(previous) = previous {
                    names.insert(name, previous.get() as usize - 1);
                } else {
                    names.remove(name);
                }
            }
        }
    }

    fn find_local(&self, name: &str) -> Option<u32> {
        if let Some(names) = &self.local_names {
            names.get(name).map(|&i| self.locals[i].1)
        } else {
            self.locals
                .iter()
                .rev()
                .find(|(n, _, _, _)| *n == name)
                .map(|(_, slot, _, _)| *slot)
        }
    }

    fn find_or_add_capture(&mut self, name: &str, cap: Capture, span: Span) -> Result<u32, Error> {
        let found = if let Some(names) = &self.capture_names {
            names.get(name).copied()
        } else {
            self.captures
                .iter()
                .position(|(n, _)| n.as_ref() == name)
                .map(|i| i as u32)
        };
        if let Some(index) = found {
            return Ok(index);
        }
        let idx = u32::try_from(self.captures.len())
            .map_err(|_| Error::parse("compile.capacity", span, "too many captured variables"))?;
        let name: Rc<str> = Rc::from(name);
        self.captures.push((Rc::clone(&name), cap));
        if let Some(names) = &mut self.capture_names {
            names.insert(name, idx);
        } else if self.captures.len() > INLINE_NAMES {
            self.capture_names = Some(Box::new(
                self.captures
                    .iter()
                    .enumerate()
                    .map(|(i, (name, _))| (Rc::clone(name), i as u32))
                    .collect(),
            ));
        }
        Ok(idx)
    }
}

impl<'a, 'p> Compiler<'a, 'p> {
    pub(super) fn new_scope(&self) -> Scope<'a> {
        let parent = self.scopes.last().unwrap();
        let mut inner = Scope::new(self.module);
        inner.outer_local_count = parent.outer_local_count + parent.locals.len();
        inner
    }

    // 最後に記録したノードまで戻り、増えた束縛だけを追加する。
    // 兄弟の関数・クロージャは同じ親ノードを参照する。
    fn visible_name_head(&mut self) -> Option<usize> {
        let mut pending_from = None;
        let mut head = None;
        'scopes: for (scope_index, scope) in self.scopes.iter().enumerate().rev() {
            for (local_index, (_, _, saved, _)) in scope.locals.iter().enumerate().rev() {
                if let Some(saved) = saved {
                    head = Some(saved.get() - 1);
                    break 'scopes;
                }
                pending_from = Some((scope_index, local_index));
            }
        }
        if let Some((first_scope, first_local)) = pending_from {
            for (index, scope) in self.scopes.iter_mut().enumerate().skip(first_scope) {
                let start = if index == first_scope { first_local } else { 0 };
                for (name, _, saved, _) in &mut scope.locals[start..] {
                    debug_assert!(saved.is_none());
                    let table = &mut self.name_table;
                    let index = table.nodes.len();
                    let start = table.text.len();
                    table.text.push_str(name);
                    table.nodes.push(NameNode {
                        start,
                        end: table.text.len(),
                        parent: head,
                    });
                    *saved = NonZeroUsize::new(index + 1);
                    head = Some(index);
                }
            }
        }
        head
    }

    // 大域の束縛は上書きされても消えない。既存の値があれば未定義名にならない。
    // 再読込で消えるモジュールの束縛は、この判定に含めない。
    // 通常の emit を小さく保ち、診断記録の処理を分離する。
    #[cold]
    pub(super) fn record_name_site(&mut self, id: u32, ip: usize) {
        let scope = self.scopes.last().unwrap();
        // ローカルが見えない場所では文字数も調べず、表の準備にも入らない。
        if scope.outer_local_count == 0 && scope.locals.is_empty() {
            return;
        }
        let Some(sym_name) = self.syms.name(id) else {
            return;
        };
        if !needs_name_candidates(sym_name) {
            return;
        }
        if self.shared_names.is_none() {
            self.shared_names = Some(Rc::new(OnceCell::new()));
        }
        if let Some(head) = self.visible_name_head() {
            let previous = self.chunk().names.as_ref().map(|names| names.last_site);
            let site = self.name_table.sites.len();
            self.name_table.sites.push(NameSite { ip, head, previous });
            if let Some(names) = &mut self.chunk().names {
                names.last_site = site;
            } else {
                let table = Rc::clone(self.shared_names.as_ref().unwrap());
                self.chunk().names = Some(NameInfo {
                    table,
                    last_site: site,
                });
            }
        }
    }

    pub(super) fn resolve_var(&mut self, name: &str, span: Span) -> Result<Op, Error> {
        let top = self.scopes.len() - 1;
        if let Some(slot) = self.scopes[top].find_local(name) {
            return Ok(Op::GetLocal(slot));
        }
        if top > 0
            && let Some(cap_idx) = self.resolve_capture(top, name, span)?
        {
            return Ok(Op::GetCapture(cap_idx));
        }
        let id = self.intern_sym(name, span)?;
        Ok(Op::GetGlobal(id))
    }

    fn resolve_capture(
        &mut self,
        scope_idx: usize,
        name: &str,
        span: Span,
    ) -> Result<Option<u32>, Error> {
        if scope_idx == 0 {
            return Ok(None);
        }
        let parent = scope_idx - 1;
        if let Some(slot) = self.scopes[parent].find_local(name) {
            let cap_idx =
                self.scopes[scope_idx].find_or_add_capture(name, Capture::Local(slot), span)?;
            return Ok(Some(cap_idx));
        }
        if parent > 0
            && let Some(parent_cap) = self.resolve_capture(parent, name, span)?
        {
            let cap_idx = self.scopes[scope_idx].find_or_add_capture(
                name,
                Capture::Upvalue(parent_cap),
                span,
            )?;
            return Ok(Some(cap_idx));
        }
        Ok(None)
    }
}
