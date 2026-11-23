use super::*;
use crate::HeapAllocator;

#[derive(Default, Debug, PartialEq)]
pub(super) struct Counts {
    pub references: usize,
    objects: usize,
    fields: usize,
}
impl Counts {
    pub fn freed(&mut self, obj: &Obj) {
        self.objects += 1;
        self.fields += match obj {
            Obj::Cons { .. } => 2,
            Obj::Array(xs) | Obj::Tuple(xs) => xs.len(),
            Obj::Struct { fields, .. } => fields.len(),
            Obj::Module { exports, .. } => exports.len(),
            Obj::Closure { captures, .. } => captures.len(),
            Obj::Some(_) | Obj::Ok(_) | Obj::Err(_) => 1,
            Obj::Variant { data, .. } => usize::from(data.is_some()),
            Obj::Str(_) => 0,
        };
    }
}

#[test]
fn release_walks_only_freed_objects_and_their_edges() {
    for n in [0, 1, 1000, 10_000] {
        let mut alloc = HeapAllocator;
        let mut pool = Pool::new(&mut alloc, usize::MAX);
        let unrelated = pool
            .alloc_obj(Obj::Str(Rc::from("keep")), Span::default())
            .unwrap();
        let baseline = pool.live_bytes();
        let mut tail = RVal::EmptyList;
        // 所有権を次の節へ移し、参照数1の鎖を組み立てる。
        for len in 1..=n {
            tail = RVal::Obj(
                pool.alloc_obj(
                    Obj::Cons {
                        head: RVal::Int(1),
                        tail,
                        len,
                    },
                    Span::default(),
                )
                .unwrap(),
            );
        }
        pool.release_val(tail);
        assert_eq!(
            pool.release_counts,
            Counts {
                references: n,
                objects: n,
                fields: 2 * n
            }
        );
        assert_eq!(pool.live_bytes(), baseline);
        assert!(matches!(pool.get(unrelated), Obj::Str(_)));
        pool.release(unrelated);
        assert_eq!(pool.live_bytes(), 0);
    }
}

#[test]
fn shared_children_are_scanned_only_on_the_last_reference() {
    for n in [1, 1000, 10_000] {
        let mut alloc = HeapAllocator;
        let mut pool = Pool::new(&mut alloc, usize::MAX);
        let shared = pool
            .alloc_obj(Obj::Array(vec![RVal::Int(1); n]), Span::default())
            .unwrap();
        let baseline = pool.live_bytes();
        for _ in 0..n {
            pool.retain(shared);
        }
        let root = pool
            .alloc_obj(Obj::Tuple(vec![RVal::Obj(shared); n]), Span::default())
            .unwrap();
        pool.release(root);
        assert_eq!(
            pool.release_counts,
            Counts {
                references: n + 1,
                objects: 1,
                fields: n
            }
        );
        assert_eq!(pool.live_bytes(), baseline);
        pool.release_counts = Counts::default();
        pool.release(shared);
        assert_eq!(
            pool.release_counts,
            Counts {
                references: 1,
                objects: 1,
                fields: n
            }
        );
        assert_eq!(pool.live_bytes(), 0);
    }
}
