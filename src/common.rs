use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Mutex;

/// A thread-safe cache for key-value pairs.
pub struct Cache<K, V> {
    inner: Lazy<Mutex<HashMap<K, V>>>,
}

impl<K, V> Default for Cache<K, V>
where
    K: Eq + Hash,
    V: Clone,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<K, V> Cache<K, V>
where
    K: Eq + Hash,
    V: Clone,
{
    pub const fn new() -> Self {
        Self {
            inner: Lazy::new(|| Mutex::new(HashMap::new())),
        }
    }

    pub fn get(&self, key: &K) -> Option<V> {
        let cache = self.inner.lock().unwrap();
        cache.get(key).cloned()
    }

    pub fn insert(&self, key: K, value: V) {
        let mut cache = self.inner.lock().unwrap();
        cache.insert(key, value);
    }
}

/// A thread-safe memoized value.
pub struct Memoized<T> {
    inner: Lazy<Mutex<Option<T>>>,
}

impl<T> Default for Memoized<T>
where
    T: Clone,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Memoized<T>
where
    T: Clone,
{
    pub const fn new() -> Self {
        Self {
            inner: Lazy::new(|| Mutex::new(None)),
        }
    }

    pub fn get(&self) -> Option<T> {
        let cache = self.inner.lock().unwrap();
        cache.clone()
    }

    pub fn set(&self, value: T) {
        let mut cache = self.inner.lock().unwrap();
        *cache = Some(value);
    }
}
