use std::collections::{HashMap, VecDeque};

pub struct LruCache<K, V> {
    map: HashMap<K, V>,
    order: VecDeque<K>,
    capacity: usize,
}

impl<K, V> LruCache<K, V>
where
    K: std::hash::Hash + Eq + Clone,
{
    pub fn new(capacity: usize) -> Self {
        Self {
            map: HashMap::new(),
            order: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    pub fn get(&mut self, key: &K) -> Option<&V> {
        if let Some(value) = self.map.get(key) {
            // Move to front (most recently used)
            self.order.retain(|k| k != key);
            self.order.push_front(key.clone());
            Some(value)
        } else {
            None
        }
    }

    pub fn put(&mut self, key: K, value: V) {
        if self.map.contains_key(&key) {
            self.map.insert(key.clone(), value);
            self.order.retain(|k| k != &key);
            self.order.push_front(key);
        } else {
            if self.map.len() >= self.capacity {
                if let Some(lru_key) = self.order.pop_back() {
                    self.map.remove(&lru_key);
                }
            }
            self.map.insert(key.clone(), value);
            self.order.push_front(key);
        }
    }

    pub fn contains(&self, key: &K) -> bool {
        self.map.contains_key(key)
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}
