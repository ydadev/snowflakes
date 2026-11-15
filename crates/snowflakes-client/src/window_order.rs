//! Bounded ordering of an enumerated inventory using explicit, revalidated Z links.
use crate::scene_registry::MAX_WINDOWS;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Owner {
    pub process: u32,
    pub thread: u32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Mark {
    pub handle: usize,
    pub owner: Owner,
}
pub(crate) trait Links {
    type Error;
    fn top(&mut self) -> Result<Option<usize>, Self::Error>;
    fn next(&mut self, handle: usize) -> Result<Option<usize>, Self::Error>;
    fn owner(&mut self, handle: usize) -> Option<Owner>;
}
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Failure<E> {
    InvalidInventory,
    Changed,
    Cycle,
    Native(E),
}

pub(crate) fn collect<S: Links>(
    inventory: &[Mark],
    source: &mut S,
) -> Result<Vec<Mark>, Failure<S::Error>> {
    if inventory.len() > MAX_WINDOWS {
        return Err(Failure::InvalidInventory);
    }
    let mut eligible = BTreeMap::new();
    for mark in inventory {
        if mark.handle == 0
            || mark.owner.process == 0
            || mark.owner.thread == 0
            || eligible.insert(mark.handle, mark.owner).is_some()
        {
            return Err(Failure::InvalidInventory);
        }
    }
    let head = source.top().map_err(Failure::Native)?;
    let mut current = head;
    let mut seen = BTreeSet::new();
    let mut walk = Vec::new();
    for _ in 0..MAX_WINDOWS {
        let Some(handle) = current else {
            break;
        };
        if !seen.insert(handle) {
            return Err(Failure::Cycle);
        }
        let owner = source.owner(handle).ok_or(Failure::Changed)?;
        current = source.next(handle).map_err(Failure::Native)?;
        if source.owner(handle) != Some(owner) {
            return Err(Failure::Changed);
        }
        walk.push(Mark { handle, owner });
    }
    // At the cap, a repeated tail is still a cycle rather than a valid long prefix.
    if current.is_some_and(|handle| seen.contains(&handle)) {
        return Err(Failure::Cycle);
    }
    let tail = current;
    if source.top().map_err(Failure::Native)? != head {
        return Err(Failure::Changed);
    }
    for (index, mark) in walk.iter().enumerate() {
        if source.owner(mark.handle) != Some(mark.owner) {
            return Err(Failure::Changed);
        }
        let expected_next = walk.get(index + 1).map(|m| m.handle).or(tail);
        if source.next(mark.handle).map_err(Failure::Native)? != expected_next
            || source.owner(mark.handle) != Some(mark.owner)
        {
            return Err(Failure::Changed);
        }
    }
    if source.top().map_err(Failure::Native)? != head {
        return Err(Failure::Changed);
    }
    if tail.is_none() {
        for mark in inventory {
            // A disappeared/reused member can be omitted. A still-live member
            // missing from a completed chain makes the observation inconsistent.
            if !seen.contains(&mark.handle) && source.owner(mark.handle) == Some(mark.owner) {
                return Err(Failure::Changed);
            }
        }
    }
    let ordered: Vec<_> = walk
        .into_iter()
        .filter(|mark| eligible.get(&mark.handle) == Some(&mark.owner))
        .collect();
    if tail.is_some() && !inventory.is_empty() && ordered.is_empty() {
        return Err(Failure::Changed);
    }
    Ok(ordered)
}

#[cfg(test)]
mod tests;
