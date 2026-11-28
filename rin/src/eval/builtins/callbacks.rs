//! Rin の関数を呼び戻す高階コレクション操作（ネイティブスタック非再帰の状態機械）。
use super::*;
use crate::mem::RawVec;

#[derive(Clone, Copy)]
pub(in crate::eval) enum CallbackKind {
    Find,
    Any,
    All,
    Map,
    Filter,
    FlatMap,
    Fold,
    SortBy,
}

impl CallbackKind {
    pub(in crate::eval) fn from_builtin(b: BuiltinId) -> Self {
        match b {
            BuiltinId::Find => Self::Find,
            BuiltinId::Any => Self::Any,
            BuiltinId::All => Self::All,
            BuiltinId::Map => Self::Map,
            BuiltinId::Filter => Self::Filter,
            BuiltinId::FlatMap => Self::FlatMap,
            BuiltinId::Fold => Self::Fold,
            BuiltinId::SortBy => Self::SortBy,
            _ => unreachable!(),
        }
    }

    pub(in crate::eval) const fn builtin_id(self) -> BuiltinId {
        match self {
            Self::Find => BuiltinId::Find,
            Self::Any => BuiltinId::Any,
            Self::All => BuiltinId::All,
            Self::Map => BuiltinId::Map,
            Self::Filter => BuiltinId::Filter,
            Self::FlatMap => BuiltinId::FlatMap,
            Self::Fold => BuiltinId::Fold,
            Self::SortBy => BuiltinId::SortBy,
        }
    }

    pub(in crate::eval) const fn name(self) -> &'static str {
        match self {
            Self::Find => "find",
            Self::Any => "any",
            Self::All => "all",
            Self::Map => "map",
            Self::Filter => "filter",
            Self::FlatMap => "flat_map",
            Self::Fold => "fold",
            Self::SortBy => "sort_by",
        }
    }
}

/// 高階組み込み関数の実行状態を保持するフレーム（作業領域上の `WorkBox` へ格納する）。
pub(in crate::eval) struct BuiltinFrame {
    pub(in crate::eval) kind: CallbackKind,
    pub(in crate::eval) span: Span,
    pub(in crate::eval) callback: RVal,
    pub(in crate::eval) items_owner: RVal,
    /// 現在処理中の要素（`head`）と、次のコンスセル（`next_cur`）。
    pub(in crate::eval) cur_item: RVal,
    pub(in crate::eval) next_cur: RVal,
    pub(in crate::eval) out: RawVec<RVal>,
    pub(in crate::eval) keyed: RawVec<(RVal, RVal)>,
    pub(in crate::eval) acc: RVal,
    pub(in crate::eval) cleanup_stack_to: usize,
}

pub(in crate::eval) enum CallbackStep {
    Done(RVal),
    Invoke {
        callee: RVal,
        args: [RVal; 2],
        argc: usize,
        span: Span,
        builtin_id: BuiltinId,
    },
}

impl Runner<'_, '_> {
    pub(in crate::eval) fn cleanup_builtin_frame(&mut self, frame: &mut BuiltinFrame) {
        self.pool
            .release_val(std::mem::replace(&mut frame.callback, RVal::Void));
        self.pool
            .release_val(std::mem::replace(&mut frame.items_owner, RVal::Void));
        self.pool
            .release_val(std::mem::replace(&mut frame.acc, RVal::Void));
        self.release_slice(frame.out.as_slice());
        frame.out.clear();
        for &(k, _) in frame.keyed.as_slice() {
            self.pool.release_val(k);
        }
        frame.keyed.clear();
    }

    pub(in crate::eval) fn start_callback_builtin(
        &mut self,
        b: BuiltinId,
        args: &[RVal],
        span: Span,
        cleanup_stack_to: usize,
    ) -> Result<(Option<BuiltinFrame>, CallbackStep), ErrRecorded> {
        let kind = CallbackKind::from_builtin(b);
        let (f, init_val, list_v) = match kind {
            CallbackKind::Fold => {
                let [f, init, list_v] = args else {
                    return Err(self
                        .callback_arity_error(span, "'fold' takes 3 arguments (fn, init, list)"));
                };
                (*f, Some(*init), *list_v)
            }
            CallbackKind::SortBy => {
                let [f, list_v] = args else {
                    return Err(self
                        .callback_arity_error(span, "'sort_by' takes 2 arguments (key_fn, list)"));
                };
                (*f, None, *list_v)
            }
            _ => {
                let [f, list_v] = args else {
                    let msg = match kind {
                        CallbackKind::Find => "'find' takes 2 arguments (fn, list)",
                        CallbackKind::Any => "'any' takes 2 arguments (fn, list)",
                        CallbackKind::All => "'all' takes 2 arguments (fn, list)",
                        CallbackKind::Map => "'map' takes 2 arguments (fn, list)",
                        CallbackKind::Filter => "'filter' takes 2 arguments (fn, list)",
                        CallbackKind::FlatMap => "'flat_map' takes 2 arguments (fn, list)",
                        _ => unreachable!(),
                    };
                    return Err(self.callback_arity_error(span, msg));
                };
                (*f, None, *list_v)
            }
        };

        self.expect_list(kind.name(), list_v, span)?;
        let list_len = self.pool.list_len(list_v);
        if !self.try_charge(list_len) {
            return self.step_limit_result(span);
        }

        let RVal::Obj(first_id) = list_v else {
            let done = match kind {
                CallbackKind::Find => RVal::None,
                CallbackKind::Any => RVal::Bool(false),
                CallbackKind::All => RVal::Bool(true),
                CallbackKind::Map
                | CallbackKind::Filter
                | CallbackKind::FlatMap
                | CallbackKind::SortBy => RVal::EmptyList,
                CallbackKind::Fold => {
                    let v = init_val.unwrap();
                    self.pool.retain_val(v);
                    v
                }
            };
            return Ok((None, CallbackStep::Done(done)));
        };
        let Obj::Cons {
            head: first_x,
            tail: next_cur,
            ..
        } = *self.pool.get(first_id)
        else {
            unreachable!();
        };

        self.pool.retain_val(f);
        self.pool.retain_val(list_v);
        let acc = if let Some(init) = init_val {
            self.pool.retain_val(init);
            init
        } else {
            RVal::Void
        };

        let (call_args, call_argc) = match kind {
            CallbackKind::Fold => ([acc, first_x], 2),
            _ => ([first_x, RVal::Void], 1),
        };

        let frame = BuiltinFrame {
            kind,
            span,
            callback: f,
            items_owner: list_v,
            cur_item: first_x,
            next_cur,
            out: RawVec::new(),
            keyed: RawVec::new(),
            acc,
            cleanup_stack_to,
        };
        let step = CallbackStep::Invoke {
            callee: f,
            args: call_args,
            argc: call_argc,
            span,
            builtin_id: kind.builtin_id(),
        };
        Ok((Some(frame), step))
    }

    #[inline(always)]
    pub(in crate::eval) fn resume_callback_builtin(
        &mut self,
        frame: &mut BuiltinFrame,
        result: RVal,
    ) -> Result<CallbackStep, ErrRecorded> {
        let span = frame.span;
        let x = frame.cur_item;
        match frame.kind {
            CallbackKind::Find => match result {
                RVal::Bool(true) => {
                    self.pool.retain_val(x);
                    let id = self.pool.alloc_obj(Obj::Some(x), span)?;
                    return Ok(CallbackStep::Done(RVal::Obj(id)));
                }
                RVal::Bool(false) => {}
                other => {
                    let tname = other.type_name(self.pool);
                    self.pool.release_val(result);
                    return Err(self.err_runtime_fmt(
                        "fn.list",
                        span,
                        format_args!("'find' predicate must return bool, got {tname}"),
                    ));
                }
            },
            CallbackKind::Any => match result {
                RVal::Bool(true) => return Ok(CallbackStep::Done(RVal::Bool(true))),
                RVal::Bool(false) => {}
                other => {
                    let tname = other.type_name(self.pool);
                    self.pool.release_val(result);
                    return Err(self.err_runtime_fmt(
                        "fn.list",
                        span,
                        format_args!("'any' predicate must return bool, got {tname}"),
                    ));
                }
            },
            CallbackKind::All => match result {
                RVal::Bool(true) => {}
                RVal::Bool(false) => return Ok(CallbackStep::Done(RVal::Bool(false))),
                other => {
                    let tname = other.type_name(self.pool);
                    self.pool.release_val(result);
                    return Err(self.err_runtime_fmt(
                        "fn.list",
                        span,
                        format_args!("'all' predicate must return bool, got {tname}"),
                    ));
                }
            },
            CallbackKind::Map => {
                if frame
                    .out
                    .push_work(&mut self.pool.work, &mut self.pool.core, result)
                    .is_none()
                {
                    self.pool.release_val(result);
                    return Err(self.work_oom(span));
                }
            }
            CallbackKind::Filter => match result {
                RVal::Bool(true) => {
                    self.pool.retain_val(x);
                    if frame
                        .out
                        .push_work(&mut self.pool.work, &mut self.pool.core, x)
                        .is_none()
                    {
                        self.pool.release_val(x);
                        return Err(self.work_oom(span));
                    }
                }
                RVal::Bool(false) => {}
                other => {
                    let tname = other.type_name(self.pool);
                    self.pool.release_val(result);
                    return Err(self.err_runtime_fmt(
                        "fn.list",
                        span,
                        format_args!("'filter' predicate must return bool, got {tname}"),
                    ));
                }
            },
            CallbackKind::FlatMap => {
                if let Err(e) = self.expect_list("flat_map", result, span) {
                    self.pool.release_val(result);
                    return Err(e);
                }
                let sub_len = self.pool.list_len(result);
                if !self.try_charge(sub_len) {
                    self.pool.release_val(result);
                    return self.step_limit_result(span);
                }
                if frame
                    .out
                    .reserve_work(&mut self.pool.work, &mut self.pool.core, sub_len)
                    .is_none()
                {
                    self.pool.release_val(result);
                    return Err(self.work_oom(span));
                }
                let mut cur = result;
                while let RVal::Obj(cid) = cur {
                    let Obj::Cons { head, tail, .. } = *self.pool.get(cid) else {
                        break;
                    };
                    self.pool.retain_val(head);
                    let _ = frame
                        .out
                        .push_work(&mut self.pool.work, &mut self.pool.core, head);
                    cur = tail;
                }
                self.pool.release_val(result);
            }
            CallbackKind::Fold => {
                self.pool.release_val(frame.acc);
                frame.acc = result;
            }
            CallbackKind::SortBy => {
                if frame
                    .keyed
                    .push_work(&mut self.pool.work, &mut self.pool.core, (result, x))
                    .is_none()
                {
                    self.pool.release_val(result);
                    return Err(self.work_oom(span));
                }
            }
        }

        if let RVal::Obj(next_id) = frame.next_cur {
            let Obj::Cons {
                head: next_x,
                tail: next_tail,
                ..
            } = *self.pool.get(next_id)
            else {
                unreachable!();
            };
            frame.cur_item = next_x;
            frame.next_cur = next_tail;
            let (call_args, call_argc) = match frame.kind {
                CallbackKind::Fold => ([frame.acc, next_x], 2),
                _ => ([next_x, RVal::Void], 1),
            };
            return Ok(CallbackStep::Invoke {
                callee: frame.callback,
                args: call_args,
                argc: call_argc,
                span,
                builtin_id: frame.kind.builtin_id(),
            });
        }

        // 全要素完了。最終値を構築する。
        let final_val = match frame.kind {
            CallbackKind::Find => RVal::None,
            CallbackKind::Any => RVal::Bool(false),
            CallbackKind::All => RVal::Bool(true),
            CallbackKind::Map | CallbackKind::Filter | CallbackKind::FlatMap => {
                let mut out = std::mem::take(&mut frame.out);
                let res = self.build_list_from_owned_raw(out.as_slice(), span);
                out.clear();
                res?
            }
            CallbackKind::Fold => std::mem::replace(&mut frame.acc, RVal::Void),
            CallbackKind::SortBy => {
                let n = frame.keyed.len();
                if !self.charge_sort(frame.keyed.as_slice().iter().map(|(k, _)| *k), n) {
                    return self.step_limit_result(span);
                }
                // 比較中は `self` の借用を分割するため、ソート用スライスへ安全にアクセスする。
                let keyed_slice = frame.keyed.as_mut_slice();
                // 挿入・マージソートは安定かつシステムヒープ不要（小さい配列または作業領域ソート）。
                self.sort_keyed_work(keyed_slice, span)?;
                let mark = self.pool.work.mark(&self.pool.core);
                let mut sorted = RawVec::<RVal>::new();
                if sorted
                    .reserve_work(&mut self.pool.work, &mut self.pool.core, n)
                    .is_none()
                {
                    self.pool.work.rewind(&mut self.pool.core, mark);
                    return Err(self.work_oom(span));
                }
                for &(_, v) in frame.keyed.as_slice() {
                    let _ = sorted.push_work(&mut self.pool.work, &mut self.pool.core, v);
                }
                let res = self.build_list_from_borrowed(sorted.as_slice(), span);
                self.pool.work.rewind(&mut self.pool.core, mark);
                res?
            }
        };
        Ok(CallbackStep::Done(final_val))
    }

    /// システムヒープを使わない自然ラン検出＋挿入ソート拡張＋ピンポン安定マージソート（`WorkArena` 使用）。
    ///
    /// 比較は常にリスト上で左の要素を第 1 引数、右の要素を第 2 引数として行う。
    /// 整列済みまたは厳密逆順の入力は O(n) の 1 パスで完了し、作業バッファ確保も不要。
    pub(in crate::eval) fn sort_keyed_work(
        &mut self,
        slice: &mut [(RVal, RVal)],
        span: Span,
    ) -> Result<(), ErrRecorded> {
        const MIN_RUN: usize = 16;
        let n = slice.len();
        if n < 2 {
            return Ok(());
        }
        let mark = self.pool.work.mark(&self.pool.core);
        let mut runs = RawVec::<(usize, usize)>::new();
        let mut idx = 0usize;
        while idx < n {
            let start = idx;
            let mut end = idx + 1;
            if end < n {
                let first_ord = match self.compare_vals(slice[start].0, slice[end].0, span) {
                    Ok(o) => o,
                    Err(e) => {
                        self.pool.work.rewind(&mut self.pool.core, mark);
                        return Err(e);
                    }
                };
                if first_ord == std::cmp::Ordering::Greater {
                    // 厳密降順ラン（安定性を保つため `Greater` のみ延長し、反転して昇順化）
                    end += 1;
                    while end < n {
                        let ord = match self.compare_vals(slice[end - 1].0, slice[end].0, span) {
                            Ok(o) => o,
                            Err(e) => {
                                self.pool.work.rewind(&mut self.pool.core, mark);
                                return Err(e);
                            }
                        };
                        if ord == std::cmp::Ordering::Greater {
                            end += 1;
                        } else {
                            break;
                        }
                    }
                    slice[start..end].reverse();
                } else {
                    // 広義昇順ラン
                    end += 1;
                    while end < n {
                        let ord = match self.compare_vals(slice[end - 1].0, slice[end].0, span) {
                            Ok(o) => o,
                            Err(e) => {
                                self.pool.work.rewind(&mut self.pool.core, mark);
                                return Err(e);
                            }
                        };
                        if ord != std::cmp::Ordering::Greater {
                            end += 1;
                        } else {
                            break;
                        }
                    }
                }
            }
            // 短いランは二分探索挿入ソートで MIN_RUN まで伸ばして比較回数とマージ段数を抑える
            let target = (start + MIN_RUN).min(n);
            if end < target {
                for cur in end..target {
                    let item = slice[cur];
                    let mut lo = start;
                    let mut hi = cur;
                    while lo < hi {
                        let mid = lo + (hi - lo) / 2;
                        let ord = match self.compare_vals(slice[mid].0, item.0, span) {
                            Ok(o) => o,
                            Err(e) => {
                                self.pool.work.rewind(&mut self.pool.core, mark);
                                return Err(e);
                            }
                        };
                        if ord == std::cmp::Ordering::Greater {
                            hi = mid;
                        } else {
                            lo = mid + 1;
                        }
                    }
                    if lo < cur {
                        slice.copy_within(lo..cur, lo + 1);
                        slice[lo] = item;
                    }
                }
                end = target;
            }
            if start == 0 && end == n {
                self.pool.work.rewind(&mut self.pool.core, mark);
                return Ok(());
            }
            if runs
                .push_work(&mut self.pool.work, &mut self.pool.core, (start, end))
                .is_none()
            {
                self.pool.work.rewind(&mut self.pool.core, mark);
                return Err(self.work_oom(span));
            }
            idx = end;
        }

        let mut scratch = RawVec::<(RVal, RVal)>::new();
        if scratch
            .resize_work(
                &mut self.pool.work,
                &mut self.pool.core,
                n,
                (RVal::Void, RVal::Void),
            )
            .is_none()
        {
            self.pool.work.rewind(&mut self.pool.core, mark);
            return Err(self.work_oom(span));
        }
        let buf = scratch.as_mut_slice();
        let mut from_slice = true;
        while runs.len() > 1 {
            let (src, dst): (&[_], &mut [_]) = if from_slice {
                (&*slice, &mut *buf)
            } else {
                (&*buf, &mut *slice)
            };
            let mut out_len = 0usize;
            let run_count = runs.len();
            let mut r = 0usize;
            while r < run_count {
                if r + 1 >= run_count {
                    let (left, right) = runs.get_copy(r);
                    dst[left..right].copy_from_slice(&src[left..right]);
                    runs.set_at(out_len, (left, right));
                    out_len += 1;
                    break;
                }
                let (left, mid) = runs.get_copy(r);
                let (_, right) = runs.get_copy(r + 1);
                let boundary = match self.compare_vals(src[mid - 1].0, src[mid].0, span) {
                    Ok(o) => o,
                    Err(e) => {
                        self.pool.work.rewind(&mut self.pool.core, mark);
                        return Err(e);
                    }
                };
                if boundary != std::cmp::Ordering::Greater {
                    dst[left..right].copy_from_slice(&src[left..right]);
                } else {
                    let mut i = left;
                    let mut j = mid;
                    let mut k = left;
                    while i < mid && j < right {
                        let ord = match self.compare_vals(src[i].0, src[j].0, span) {
                            Ok(o) => o,
                            Err(e) => {
                                self.pool.work.rewind(&mut self.pool.core, mark);
                                return Err(e);
                            }
                        };
                        if ord != std::cmp::Ordering::Greater {
                            dst[k] = src[i];
                            i += 1;
                        } else {
                            dst[k] = src[j];
                            j += 1;
                        }
                        k += 1;
                    }
                    if i < mid {
                        let rem = mid - i;
                        dst[k..k + rem].copy_from_slice(&src[i..mid]);
                        k += rem;
                    }
                    if j < right {
                        let rem = right - j;
                        dst[k..k + rem].copy_from_slice(&src[j..right]);
                    }
                }
                runs.set_at(out_len, (left, right));
                out_len += 1;
                r += 2;
            }
            runs.truncate(out_len);
            from_slice = !from_slice;
        }
        if !from_slice {
            slice.copy_from_slice(buf);
        }
        self.pool.work.rewind(&mut self.pool.core, mark);
        Ok(())
    }

    #[cold]
    #[inline(never)]
    fn callback_arity_error(&mut self, span: Span, message: &'static str) -> ErrRecorded {
        self.err_runtime("fn.list", span, message)
    }
}
