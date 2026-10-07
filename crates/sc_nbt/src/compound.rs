//! `CompoundNbt`: NBT compound tag (key-value container with insertion-order semantics).
//!
//! Provides get/insert/remove/iter operations; unknown fields are preserved verbatim for lossless save round-trips.

use crate::NbtValue;
use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct CompoundNbt {
    pub name: Option<String>,
    // NBT compounds are frequently cloned into block registries/dictionaries
    // and then read-only. Share their map until a caller mutates one clone.
    map: Arc<HashMap<String, NbtValue>>,
}

impl CompoundNbt {
    pub fn new(name: Option<String>) -> Self {
        Self {
            name,
            map: Arc::new(HashMap::new()),
        }
    }

    pub fn new_with_value(name: Option<String>, key: &str, value: NbtValue) -> Self {
        Self {
            name,
            map: Arc::new(HashMap::from([(key.to_string(), value)])),
        }
    }

    pub fn from_map(name: Option<String>, map: HashMap<String, NbtValue>) -> Self {
        Self {
            name,
            map: Arc::new(map),
        }
    }

    pub fn get(&self, key: &str) -> Option<&NbtValue> {
        self.map.get(key)
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut NbtValue> {
        if !self.map.contains_key(key) {
            return None;
        }
        Arc::make_mut(&mut self.map).get_mut(key)
    }

    pub fn insert(&mut self, key: &str, value: NbtValue) -> &mut Self {
        Arc::make_mut(&mut self.map).insert(key.to_string(), value);
        self
    }

    pub fn remove(&mut self, key: &str) -> Option<NbtValue> {
        if !self.map.contains_key(key) {
            return None;
        }
        Arc::make_mut(&mut self.map).remove(key)
    }

    pub fn contains_key(&self, key: &str) -> bool {
        self.map.contains_key(key)
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &NbtValue)> {
        let mut entries = self.map.iter().collect::<Vec<_>>();
        entries.sort_by(|a, b| a.0.cmp(b.0));
        entries.into_iter()
    }

    pub fn extend(&mut self, other: &CompoundNbt) -> &mut Self {
        Arc::make_mut(&mut self.map).extend(
            other
                .map
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clones_share_read_only_storage_and_mutations_remain_isolated() {
        let mut original = CompoundNbt::new(None);
        original.insert("age", NbtValue::Int(3));
        original.insert("name", NbtValue::String("oak".into()));

        let mut clone = original.clone();
        assert!(Arc::ptr_eq(&original.map, &clone.map));

        clone.insert("age", NbtValue::Int(4));
        assert!(!Arc::ptr_eq(&original.map, &clone.map));
        assert!(matches!(original.get("age"), Some(NbtValue::Int(3))));
        assert!(matches!(clone.get("age"), Some(NbtValue::Int(4))));

        *clone.get_mut("name").expect("name") = NbtValue::String("birch".into());
        assert!(matches!(
            original.get("name"),
            Some(NbtValue::String(name)) if name == "oak"
        ));
        assert!(matches!(
            clone.get("name"),
            Some(NbtValue::String(name)) if name == "birch"
        ));
    }

    #[test]
    fn extend_preserves_existing_copy_semantics() {
        let mut left = CompoundNbt::new(None);
        left.insert("left", NbtValue::Int(1));
        let mut right = CompoundNbt::new(None);
        right.insert("right", NbtValue::Int(2));

        left.extend(&right);
        assert!(matches!(left.get("left"), Some(NbtValue::Int(1))));
        assert!(matches!(left.get("right"), Some(NbtValue::Int(2))));
        assert!(matches!(right.get("right"), Some(NbtValue::Int(2))));
    }

    #[test]
    fn missing_mutations_do_not_detach_shared_storage() {
        let mut original = CompoundNbt::new(None);
        original.insert("present", NbtValue::Int(1));
        let mut clone = original.clone();
        assert!(clone.get_mut("missing").is_none());
        assert!(clone.remove("missing").is_none());
        assert!(Arc::ptr_eq(&original.map, &clone.map));
    }
}

impl Serialize for CompoundNbt {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(Some(self.map.len()))?;
        for (key, value) in self.iter() {
            map.serialize_entry(key, value)?;
        }
        map.end()
    }
}
