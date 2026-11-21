//! Rc の最後の所有者でのみ行う、子を持つ値の解放。
use super::*;

/// 配列・タプルの要素。最後の所有者が解放するときだけ子を処理する。
#[derive(Clone, Debug, PartialEq)]
pub struct ValueItems(pub(super) Vec<Value>);
impl From<Vec<Value>> for ValueItems {
    fn from(items: Vec<Value>) -> Self {
        Self(items)
    }
}
impl std::ops::Deref for ValueItems {
    type Target = Vec<Value>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl std::ops::DerefMut for ValueItems {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
impl Drop for ValueItems {
    #[inline]
    fn drop(&mut self) {
        drop::items(&mut self.0);
    }
}

/// 構造体のフィールド。最後の所有者だけが子を解放する。
#[derive(Clone, Debug, PartialEq)]
pub struct ValueFields(pub(super) BTreeMap<String, Value>);
impl From<BTreeMap<String, Value>> for ValueFields {
    fn from(fields: BTreeMap<String, Value>) -> Self {
        Self(fields)
    }
}
impl std::ops::Deref for ValueFields {
    type Target = BTreeMap<String, Value>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl std::ops::DerefMut for ValueFields {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
impl Drop for ValueFields {
    #[inline]
    fn drop(&mut self) {
        drop::fields(&mut self.0);
    }
}

/// option・result・variant が所有する子。
#[derive(Clone, Debug, PartialEq)]
pub struct ValueChild(pub(super) Value);
impl From<Value> for ValueChild {
    fn from(value: Value) -> Self {
        Self(value)
    }
}
impl std::ops::Deref for ValueChild {
    type Target = Value;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl std::ops::DerefMut for ValueChild {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
impl Drop for ValueChild {
    #[inline]
    fn drop(&mut self) {
        drop::child(&mut self.0);
    }
}
