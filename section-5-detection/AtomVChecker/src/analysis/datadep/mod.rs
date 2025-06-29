//! BSD 3-Clause License
//!
//! Copyright (c) 2022, Boqin Qin(秦 伯钦)
//! All rights reserved.
//!
//! Check if a Local var A is (arithmetically) data-dependent on another Local var B
//! by tracking move, copy and arithmetic statements from A to B.
//! For now this analysis is limited to intraprocedural analysis and
//! is for atomicity violation detector only.

extern crate rustc_data_structures;
extern crate rustc_index;
extern crate rustc_middle;

use rustc_middle::ty::{TyCtxt, TyKind};
use std::collections::VecDeque;

use crate::analysis::callgraph::{CallGraph, InstanceId};
use crate::interest::memory::ownership;
use petgraph::Direction::Outgoing;
use rustc_data_structures::fx::FxHashSet;
use rustc_index::vec::IndexVec;
use rustc_middle::mir::visit::Visitor;
use rustc_middle::mir::{Body, Local, Location, Operand, Place, Rvalue, TerminatorKind};

pub fn all_data_dep_on<'tcx>(
    a: Local,
    data_deps: &DataDeps,
    callgraph: &CallGraph,
    instance_id: InstanceId,
    body: &Body<'tcx>,
    tcx: TyCtxt<'tcx>,
) -> FxHashSet<Local> {
    let mut worklist = VecDeque::from_iter(data_deps.immediate_dep(a).into_iter());
    let mut visited = FxHashSet::default();

    while let Some(n) = worklist.pop_front() {
        if !visited.insert(n) {
            continue;
        }
        for succ in data_deps.immediate_dep(n).into_iter() {
            worklist.push_front(succ);
        }
    }
    let mut callsites = Vec::new();
    let call_targets: Vec<InstanceId> = callgraph
        .graph
        .neighbors_directed(instance_id, Outgoing)
        .collect();
    for call_target in call_targets {
        let sites = callsite_locations(callgraph, instance_id, call_target).unwrap();
        for callsite in sites {
            callsites.push(callsite);
        }
    }
    for location in callsites {
        if let TerminatorKind::Call {
            func,
            args,
            destination,
            ..
        } = &body[location.block].terminator().kind
        {
            if let (&[Operand::Move(arg) | Operand::Copy(arg)], dest) =
                (args.as_slice(), destination)
            {
                if visited.contains(&arg.local) {
                    visited.insert(dest.local);
                }
            }
            let func_ty = func.ty(body, tcx);
            match func_ty.kind() {
                TyKind::FnDef(def_id, _) => {
                    if ownership::is_get_unchecked(*def_id, tcx)
                        || ownership::is_atomic_operate(*def_id, tcx)
                        || ownership::is_addr(*def_id, tcx)
                        || ownership::is_ptr_operate(*def_id, tcx)
                    {
                        if let Operand::Move(arg) | Operand::Copy(arg) = args[0] {
                            if visited.contains(&arg.local) {
                                visited.insert(destination.local);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    visited.insert(a);
    visited
}

pub fn data_deps(body: &Body<'_>) -> DataDeps {
    let local_num = body.local_decls.len();
    let v = IndexVec::from_elem_n(false, local_num);
    let immediate_deps = IndexVec::from_elem_n(v, local_num);
    let mut data_deps = DataDeps { immediate_deps };
    data_deps.visit_body(body);
    data_deps
}

#[derive(Clone, Debug)]
pub struct DataDeps {
    immediate_deps: IndexVec<Local, IndexVec<Local, bool>>,
}

impl DataDeps {
    fn immediate_dep(&self, local: Local) -> FxHashSet<Local> {
        self.immediate_deps[local]
            .iter_enumerated()
            .filter_map(|(local, v)| if *v { Some(local) } else { None })
            .collect()
    }
}

impl<'tcx> Visitor<'tcx> for DataDeps {
    fn visit_assign(&mut self, place: &Place<'tcx>, rvalue: &Rvalue<'tcx>, location: Location) {
        let lhs = place.local;
        match rvalue {
            Rvalue::Use(operand) => {
                // | Rvalue::Cast(_, operand, _) | Rvalue::UnaryOp(_, operand)
                if let Some(rhs) = operand.place() {
                    self.immediate_deps[rhs.local][lhs] = true;
                }
            }
            Rvalue::Cast(_, operand, _) => {
                if let Some(rhs) = operand.place() {
                    self.immediate_deps[rhs.local][lhs] = true;
                }
            }
            Rvalue::UnaryOp(_, operand) => {
                if let Some(rhs) = operand.place() {
                    self.immediate_deps[rhs.local][lhs] = true;
                }
            }
            Rvalue::BinaryOp(_, box (rhs0, rhs1)) => {
                if let Some(rhs0) = rhs0.place() {
                    self.immediate_deps[rhs0.local][lhs] = true;
                }
                if let Some(rhs1) = rhs1.place() {
                    self.immediate_deps[rhs1.local][lhs] = true;
                }
            }
            Rvalue::CheckedBinaryOp(_, box (rhs0, rhs1)) => {
                if let Some(rhs0) = rhs0.place() {
                    self.immediate_deps[rhs0.local][lhs] = true;
                }
                if let Some(rhs1) = rhs1.place() {
                    self.immediate_deps[rhs1.local][lhs] = true;
                }
            }
            _ => {}
        }
        self.super_assign(place, rvalue, location);
    }
}

pub fn callsite_locations(
    callgraph: &CallGraph<'_>,
    source: InstanceId,
    target: InstanceId,
) -> Option<Vec<Location>> {
    Some(
        callgraph
            .callsites(source, target)?
            .into_iter()
            .filter_map(|callsite| callsite.location())
            .collect(),
    )
}
