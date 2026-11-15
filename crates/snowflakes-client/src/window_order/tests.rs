use super::*;

fn mark(handle: usize) -> Mark {
    Mark {
        handle,
        owner: Owner {
            process: 100,
            thread: handle as u32,
        },
    }
}
struct Desktop {
    order: Vec<usize>,
    owners: BTreeMap<usize, Owner>,
    next_calls: usize,
    top_calls: usize,
    cycle: Option<(usize, usize)>,
    change_at: Option<usize>,
    fail_at: Option<usize>,
    owner_calls: usize,
    owner_change_at: Option<(usize, usize)>,
}
impl Desktop {
    fn new(order: Vec<usize>) -> Self {
        Self {
            owners: order.iter().map(|&h| (h, mark(h).owner)).collect(),
            order,
            next_calls: 0,
            top_calls: 0,
            cycle: None,
            change_at: None,
            fail_at: None,
            owner_calls: 0,
            owner_change_at: None,
        }
    }
}
impl Links for Desktop {
    type Error = ();
    fn top(&mut self) -> Result<Option<usize>, ()> {
        self.top_calls += 1;
        Ok(self.order.first().copied())
    }
    fn next(&mut self, handle: usize) -> Result<Option<usize>, ()> {
        self.next_calls += 1;
        if self.fail_at == Some(self.next_calls) {
            return Err(());
        }
        if self.change_at == Some(self.next_calls) {
            self.order.reverse();
        }
        if let Some((from, to)) = self.cycle
            && from == handle
        {
            return Ok(Some(to));
        }
        Ok(self
            .order
            .iter()
            .position(|&h| h == handle)
            .and_then(|i| self.order.get(i + 1))
            .copied())
    }
    fn owner(&mut self, handle: usize) -> Option<Owner> {
        self.owner_calls += 1;
        if let Some((at, changed)) = self.owner_change_at
            && at == self.owner_calls
        {
            self.owners.get_mut(&changed).unwrap().process += 1;
        }
        self.owners.get(&handle).copied()
    }
}

#[test]
fn ownership_change_during_either_pass_never_publishes_a_mixed_generation() {
    for change in [(2, 1), (7, 1), (9, 2)] {
        let mut desktop = Desktop::new(vec![1, 2, 3]);
        desktop.owner_change_at = Some(change);
        assert_eq!(
            collect(&[mark(1), mark(2), mark(3)], &mut desktop),
            Err(Failure::Changed)
        );
    }
    let mut desktop = Desktop::new(vec![1]);
    desktop.owners.clear();
    assert_eq!(collect(&[mark(1)], &mut desktop), Err(Failure::Changed));
}

#[test]
fn arbitrary_enumeration_order_is_replaced_by_explicit_front_to_back_links() {
    for order in [vec![1, 2, 3], vec![3, 1, 2], vec![2, 3, 1], vec![3, 2, 1]] {
        let mut desktop = Desktop::new(order.clone());
        let result = collect(&[mark(2), mark(1), mark(3)], &mut desktop).unwrap();
        assert_eq!(result.iter().map(|m| m.handle).collect::<Vec<_>>(), order);
        assert_eq!(desktop.next_calls, 6);
        assert_eq!(desktop.top_calls, 3);
    }
}

#[test]
fn vanished_reused_and_new_unenumerated_windows_cannot_inherit_inventory_membership() {
    let mut desktop = Desktop::new(vec![8, 1, 2, 3]);
    desktop.owners.get_mut(&2).unwrap().process = 200; // same handle, another process
    let result = collect(&[mark(1), mark(2), mark(3), mark(4)], &mut desktop).unwrap();
    assert_eq!(result, [mark(1), mark(3)]); // 4 vanished; 8 was not enumerated
}

#[test]
fn prematurely_terminated_chain_cannot_silently_drop_a_live_enumerated_window() {
    let mut desktop = Desktop::new(vec![1, 2]);
    desktop.owners.insert(3, mark(3).owner);
    assert_eq!(
        collect(&[mark(1), mark(2), mark(3)], &mut desktop),
        Err(Failure::Changed)
    );
    let mut empty = Desktop::new(vec![]);
    assert!(collect(&[], &mut empty).unwrap().is_empty());
}

#[test]
fn changes_during_capture_or_validation_and_native_errors_are_rejected() {
    for change in [1, 2, 4, 5] {
        let mut desktop = Desktop::new(vec![1, 2, 3]);
        desktop.change_at = Some(change);
        assert!(collect(&[mark(1), mark(2), mark(3)], &mut desktop).is_err());
    }
    let mut desktop = Desktop::new(vec![1, 2, 3]);
    desktop.fail_at = Some(4);
    assert_eq!(
        collect(&[mark(1), mark(2), mark(3)], &mut desktop),
        Err(Failure::Native(()))
    );
}

#[test]
fn cycles_invalid_inventory_and_capacity_edges_have_finite_work() {
    for (from, to) in [(1, 1), (3, 1), (3, 2)] {
        let mut desktop = Desktop::new(vec![1, 2, 3]);
        desktop.cycle = Some((from, to));
        assert_eq!(
            collect(&[mark(1), mark(2), mark(3)], &mut desktop),
            Err(Failure::Cycle)
        );
        assert!(desktop.next_calls <= 3);
    }
    for inventory in [
        vec![mark(0)],
        vec![mark(1), mark(1)],
        (1..=MAX_WINDOWS + 1).map(mark).collect(),
    ] {
        let mut desktop = Desktop::new(vec![1]);
        assert_eq!(
            collect(&inventory, &mut desktop),
            Err(Failure::InvalidInventory)
        );
        assert_eq!(desktop.top_calls, 0);
    }
    for count in [MAX_WINDOWS - 1, MAX_WINDOWS, MAX_WINDOWS + 1] {
        let mut desktop = Desktop::new((1..=count).collect());
        let inventory: Vec<_> = (1..=count.min(MAX_WINDOWS)).rev().map(mark).collect();
        let result = collect(&inventory, &mut desktop).unwrap();
        assert_eq!(
            result,
            (1..=count.min(MAX_WINDOWS)).map(mark).collect::<Vec<_>>()
        );
        assert_eq!(desktop.next_calls, 2 * count.min(MAX_WINDOWS));
    }
    let mut desktop = Desktop::new((1..=MAX_WINDOWS).collect());
    desktop.cycle = Some((MAX_WINDOWS, 1));
    assert_eq!(
        collect(
            &(1..=MAX_WINDOWS).map(mark).collect::<Vec<_>>(),
            &mut desktop
        ),
        Err(Failure::Cycle)
    );
    assert_eq!(desktop.next_calls, MAX_WINDOWS);
    let mut desktop = Desktop::new((1..=MAX_WINDOWS + 1).collect());
    assert_eq!(
        collect(&[mark(MAX_WINDOWS + 1)], &mut desktop),
        Err(Failure::Changed)
    );
    assert!(desktop.next_calls <= MAX_WINDOWS * 2);
}
