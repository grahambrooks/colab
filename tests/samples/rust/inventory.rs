//! A small inventory service: the property suite's Rust sample.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::path::{Path, PathBuf};

/// How an item is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Storage {
    Shelf,
    Cold { celsius: i32 },
    Bulk(u32),
}

/// One stock-keeping unit.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub sku: String,
    pub name: String,
    pub price_cents: u64,
    pub storage: Storage,
    tags: Vec<String>,
}

impl Item {
    pub fn new(sku: &str, name: &str, price_cents: u64) -> Self {
        Self {
            sku: sku.to_string(),
            name: name.to_string(),
            price_cents,
            storage: Storage::Shelf,
            tags: Vec::new(),
        }
    }

    pub fn with_storage(mut self, storage: Storage) -> Self {
        self.storage = storage;
        self
    }

    pub(crate) fn tag(&mut self, tag: impl Into<String>) {
        let tag = tag.into();
        if !self.tags.contains(&tag) {
            self.tags.push(tag);
        }
    }

    fn is_cold(&self) -> bool {
        matches!(self.storage, Storage::Cold { .. })
    }
}

impl fmt::Display for Item {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({}): {}.{:02}", self.name, self.sku, self.price_cents / 100, self.price_cents % 100)
    }
}

/// Why a lookup failed.
#[derive(Debug)]
pub enum InventoryError {
    Missing(String),
    Io { path: PathBuf, source: std::io::Error },
}

impl fmt::Display for InventoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InventoryError::Missing(sku) => write!(f, "no item {sku}"),
            InventoryError::Io { path, source } => write!(f, "{}: {source}", path.display()),
        }
    }
}

pub type Result<T> = std::result::Result<T, InventoryError>;

/// Items by SKU, with counts.
#[derive(Default)]
pub struct Inventory {
    items: HashMap<String, Item>,
    counts: BTreeMap<String, u32>,
}

impl Inventory {
    pub fn add(&mut self, item: Item, count: u32) {
        *self.counts.entry(item.sku.clone()).or_insert(0) += count;
        self.items.insert(item.sku.clone(), item);
    }

    pub fn get(&self, sku: &str) -> Result<&Item> {
        self.items
            .get(sku)
            .ok_or_else(|| InventoryError::Missing(sku.to_string()))
    }

    pub fn take(&mut self, sku: &str, count: u32) -> Result<u32> {
        let left = self
            .counts
            .get_mut(sku)
            .ok_or_else(|| InventoryError::Missing(sku.to_string()))?;
        let taken = count.min(*left);
        *left -= taken;
        Ok(taken)
    }

    pub fn total_value(&self) -> u64 {
        self.items
            .values()
            .map(|item| item.price_cents * u64::from(self.counts.get(&item.sku).copied().unwrap_or(0)))
            .sum()
    }

    pub fn cold_items(&self) -> Vec<&Item> {
        let mut cold: Vec<&Item> = self.items.values().filter(|item| item.is_cold()).collect();
        cold.sort_by(|a, b| a.sku.cmp(&b.sku));
        cold
    }

    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).map_err(|source| InventoryError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let mut inventory = Self::default();
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let fields: Vec<&str> = line.split(',').map(str::trim).collect();
            if let [sku, name, price, count] = fields.as_slice() {
                let price: u64 = price.parse().unwrap_or(0);
                let count: u32 = count.parse().unwrap_or(1);
                inventory.add(Item::new(sku, name, price), count);
            }
        }
        Ok(inventory)
    }
}

/// Something that can be priced.
pub trait Priced {
    fn price(&self) -> u64;

    fn discounted(&self, percent: u64) -> u64 {
        self.price() * (100 - percent.min(100)) / 100
    }
}

impl Priced for Item {
    fn price(&self) -> u64 {
        self.price_cents
    }
}

macro_rules! sku {
    ($prefix:literal, $n:expr) => {
        format!("{}-{:04}", $prefix, $n)
    };
}

pub fn restock<I>(inventory: &mut Inventory, items: I) -> usize
where
    I: IntoIterator<Item = (Item, u32)>,
{
    let mut added = 0;
    for (item, count) in items {
        if count == 0 {
            continue;
        }
        inventory.add(item, count);
        added += 1;
    }
    added
}

pub fn describe(storage: Storage) -> &'static str {
    match storage {
        Storage::Shelf => "shelf",
        Storage::Cold { celsius } if celsius < 0 => "freezer",
        Storage::Cold { .. } => "fridge",
        Storage::Bulk(n) if n > 1000 => "warehouse",
        Storage::Bulk(_) => "bulk",
    }
}

pub fn sample() -> Inventory {
    let mut inventory = Inventory::default();
    let milk = Item::new(&sku!("MLK", 1), "Milk", 129).with_storage(Storage::Cold { celsius: 4 });
    let rice = Item::new(&sku!("RCE", 2), "Rice", 349).with_storage(Storage::Bulk(2500));
    inventory.add(milk, 12);
    inventory.add(rice, 40);
    assert_eq!(inventory.take("MLK-0001", 2).unwrap_or(0), 2);
    debug_assert!(inventory.total_value() > 0, "an empty sample");
    inventory
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_is_capped_by_the_count() {
        let mut inventory = sample();
        assert_eq!(inventory.take("RCE-0002", 100).unwrap(), 40);
        assert!(inventory.get("nope").is_err());
        assert_eq!(describe(Storage::Cold { celsius: -18 }), "freezer");
    }
}
