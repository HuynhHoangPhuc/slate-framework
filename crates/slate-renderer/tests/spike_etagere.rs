//! Throwaway spike (Phase 3 step 1): pin down etagere 0.2.4 API surface
//! before writing atlas.rs. Confirms:
//! - `size2` import path
//! - `Allocation { id, rectangle }` field names
//! - `Option<Allocation>` return shape
//! - whether `etagere::AllocId` implements Hash + Eq (it does as of 0.2.4 —
//!   it's a `NonZeroU32` newtype, which derives both).

use std::collections::HashMap;

use etagere::{AtlasAllocator, euclid::size2};

#[test]
fn spike_alloc_and_dealloc() {
    let mut a = AtlasAllocator::new(size2(2048, 2048));
    let alloc = a.allocate(size2(64, 64)).expect("alloc 64x64");
    let id = alloc.id;
    let r = alloc.rectangle;

    // Field shape used by atlas.rs.
    let _x: i32 = r.min.x;
    let _y: i32 = r.min.y;
    let _w: i32 = r.max.x - r.min.x;
    let _h: i32 = r.max.y - r.min.y;

    // AllocId Hash+Eq → usable as HashMap key.
    let mut map: HashMap<etagere::AllocId, ()> = HashMap::new();
    map.insert(id, ());
    assert!(map.contains_key(&id));

    a.deallocate(id);
}

#[test]
fn spike_returns_none_when_full() {
    let mut a = AtlasAllocator::new(size2(64, 64));
    let _ = a.allocate(size2(64, 64)).expect("first fits");
    assert!(a.allocate(size2(64, 64)).is_none(), "second must fail");
}
