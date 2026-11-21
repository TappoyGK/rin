//! 小さい処理ではヒープ確保せず、深い処理だけ Vec に退避する作業スタック。

pub(crate) struct Work<T, const N: usize = 8> {
    local: [Option<T>; N],
    len: usize,
    overflow: Vec<T>,
}
impl<T, const N: usize> Work<T, N> {
    pub fn new() -> Self {
        Self {
            local: std::array::from_fn(|_| None),
            len: 0,
            overflow: Vec::new(),
        }
    }
    pub fn with(item: T) -> Self {
        let mut work = Self::new();
        work.push(item);
        work
    }
    pub fn push(&mut self, item: T) {
        if self.len < N {
            self.local[self.len] = Some(item);
            self.len += 1;
        } else {
            self.overflow.push(item);
        }
    }
    pub fn pop(&mut self) -> Option<T> {
        if let Some(item) = self.overflow.pop() {
            return Some(item);
        }
        if self.len == 0 {
            return None;
        }
        self.len -= 1;
        self.local[self.len].take()
    }
}
impl<T, const N: usize> Extend<T> for Work<T, N> {
    fn extend<I: IntoIterator<Item = T>>(&mut self, items: I) {
        for item in items {
            self.push(item);
        }
    }
}
