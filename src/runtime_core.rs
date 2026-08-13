//! Bounded runtime primitives shared by the ECMAScript and web-platform layers.
//!
//! This module deliberately owns no OS, network or filesystem authority. Host
//! capabilities are represented by typed heap objects and queued callback ids.

use std::collections::{BTreeMap, VecDeque};

const OBJECT_BASE_BYTES: usize = 64;
const PROPERTY_BASE_BYTES: usize = 32;
const MAX_PROPERTY_NAME_BYTES: usize = 256;
const MAX_DOM_STRING_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeLimits {
    pub max_heap_bytes: usize,
    pub max_objects: usize,
    pub max_properties_per_object: usize,
    pub max_tasks: usize,
    pub max_microtasks: usize,
    pub max_call_depth: usize,
    pub max_instructions: u64,
}

impl Default for RuntimeLimits {
    fn default() -> Self {
        Self {
            max_heap_bytes: 32 * 1024 * 1024,
            max_objects: 50_000,
            max_properties_per_object: 1_024,
            max_tasks: 2_048,
            max_microtasks: 2_048,
            max_call_depth: 128,
            max_instructions: 1_000_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HeapHandle {
    slot: u32,
    generation: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostObjectKind {
    Ordinary,
    Global,
    Document,
    Element,
    Promise,
    MediaElement,
    MediaSource,
    SourceBuffer,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RuntimeValue {
    Undefined,
    Null,
    Boolean(bool),
    Number(f64),
    String(String),
    Object(HeapHandle),
}

#[derive(Debug, Clone)]
pub struct HeapObject {
    pub kind: HostObjectKind,
    pub prototype: Option<HeapHandle>,
    properties: BTreeMap<String, RuntimeValue>,
}

impl HeapObject {
    pub fn property(&self, name: &str) -> Option<&RuntimeValue> {
        self.properties.get(name)
    }

    pub fn property_count(&self) -> usize {
        self.properties.len()
    }
}

#[derive(Debug)]
struct HeapSlot {
    generation: u32,
    marked: bool,
    bytes: usize,
    object: Option<HeapObject>,
}

#[derive(Debug)]
pub struct BoundedHeap {
    limits: RuntimeLimits,
    slots: Vec<HeapSlot>,
    free_slots: Vec<u32>,
    used_bytes: usize,
    live_objects: usize,
}

impl BoundedHeap {
    pub fn new(limits: RuntimeLimits) -> Self {
        Self {
            limits,
            slots: Vec::new(),
            free_slots: Vec::new(),
            used_bytes: 0,
            live_objects: 0,
        }
    }

    pub fn allocate(&mut self, kind: HostObjectKind) -> Result<HeapHandle, String> {
        if self.live_objects >= self.limits.max_objects {
            return Err("Runtime object budget exceeded".to_string());
        }
        if self.used_bytes.saturating_add(OBJECT_BASE_BYTES) > self.limits.max_heap_bytes {
            return Err("Runtime heap byte budget exceeded".to_string());
        }

        let object = HeapObject {
            kind,
            prototype: None,
            properties: BTreeMap::new(),
        };
        let handle = if let Some(slot_index) = self.free_slots.pop() {
            let slot = &mut self.slots[slot_index as usize];
            slot.marked = false;
            slot.bytes = OBJECT_BASE_BYTES;
            slot.object = Some(object);
            HeapHandle {
                slot: slot_index,
                generation: slot.generation,
            }
        } else {
            let slot = u32::try_from(self.slots.len())
                .map_err(|_| "Runtime heap handle space exhausted".to_string())?;
            self.slots.push(HeapSlot {
                generation: 0,
                marked: false,
                bytes: OBJECT_BASE_BYTES,
                object: Some(object),
            });
            HeapHandle {
                slot,
                generation: 0,
            }
        };
        self.used_bytes += OBJECT_BASE_BYTES;
        self.live_objects += 1;
        Ok(handle)
    }

    pub fn get(&self, handle: HeapHandle) -> Option<&HeapObject> {
        let slot = self.slots.get(handle.slot as usize)?;
        (slot.generation == handle.generation)
            .then_some(slot.object.as_ref())
            .flatten()
    }

    pub fn get_mut(&mut self, handle: HeapHandle) -> Option<&mut HeapObject> {
        let slot = self.slots.get_mut(handle.slot as usize)?;
        (slot.generation == handle.generation)
            .then_some(slot.object.as_mut())
            .flatten()
    }

    pub fn set_prototype(
        &mut self,
        handle: HeapHandle,
        prototype: Option<HeapHandle>,
    ) -> Result<(), String> {
        if prototype.is_some_and(|candidate| self.get(candidate).is_none()) {
            return Err("Prototype handle is stale".to_string());
        }
        let object = self
            .get_mut(handle)
            .ok_or_else(|| "Object handle is stale".to_string())?;
        object.prototype = prototype;
        Ok(())
    }

    pub fn set_property(
        &mut self,
        handle: HeapHandle,
        name: &str,
        value: RuntimeValue,
    ) -> Result<(), String> {
        if name.is_empty() || name.len() > MAX_PROPERTY_NAME_BYTES {
            return Err("Invalid runtime property name".to_string());
        }
        if matches!(&value, RuntimeValue::String(text) if text.len() > MAX_DOM_STRING_BYTES) {
            return Err("Runtime string exceeds 1 MB".to_string());
        }

        let (old_cost, property_count, contains_property) = {
            let object = self
                .get(handle)
                .ok_or_else(|| "Object handle is stale".to_string())?;
            (
                object
                    .properties
                    .get(name)
                    .map(|old| property_cost(name, old))
                    .unwrap_or_default(),
                object.properties.len(),
                object.properties.contains_key(name),
            )
        };
        if !contains_property && property_count >= self.limits.max_properties_per_object {
            return Err("Runtime property budget exceeded".to_string());
        }
        let new_cost = property_cost(name, &value);
        let projected = self
            .used_bytes
            .saturating_sub(old_cost)
            .saturating_add(new_cost);
        if projected > self.limits.max_heap_bytes {
            return Err("Runtime heap byte budget exceeded".to_string());
        }

        let slot = self
            .slots
            .get_mut(handle.slot as usize)
            .ok_or_else(|| "Object handle is stale".to_string())?;
        if slot.generation != handle.generation {
            return Err("Object handle is stale".to_string());
        }
        let object = slot
            .object
            .as_mut()
            .ok_or_else(|| "Object handle is stale".to_string())?;
        object.properties.insert(name.to_string(), value);
        slot.bytes = slot.bytes.saturating_sub(old_cost).saturating_add(new_cost);
        self.used_bytes = projected;
        Ok(())
    }

    pub fn remove_property(
        &mut self,
        handle: HeapHandle,
        name: &str,
    ) -> Result<Option<RuntimeValue>, String> {
        let slot = self
            .slots
            .get_mut(handle.slot as usize)
            .ok_or_else(|| "Object handle is stale".to_string())?;
        if slot.generation != handle.generation {
            return Err("Object handle is stale".to_string());
        }
        let object = slot
            .object
            .as_mut()
            .ok_or_else(|| "Object handle is stale".to_string())?;
        let Some(value) = object.properties.remove(name) else {
            return Ok(None);
        };
        let cost = property_cost(name, &value);
        slot.bytes = slot.bytes.saturating_sub(cost);
        self.used_bytes = self.used_bytes.saturating_sub(cost);
        Ok(Some(value))
    }

    pub fn collect_garbage(&mut self, roots: &[HeapHandle]) -> usize {
        for slot in &mut self.slots {
            slot.marked = false;
        }
        let mut pending = roots.to_vec();
        while let Some(handle) = pending.pop() {
            let Some(slot) = self.slots.get_mut(handle.slot as usize) else {
                continue;
            };
            if slot.generation != handle.generation || slot.marked {
                continue;
            }
            let Some(object) = slot.object.as_ref() else {
                continue;
            };
            slot.marked = true;
            if let Some(prototype) = object.prototype {
                pending.push(prototype);
            }
            pending.extend(object.properties.values().filter_map(|value| match value {
                RuntimeValue::Object(reference) => Some(*reference),
                _ => None,
            }));
        }

        let mut reclaimed = 0;
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if slot.object.is_some() && !slot.marked {
                reclaimed += slot.bytes;
                self.used_bytes = self.used_bytes.saturating_sub(slot.bytes);
                self.live_objects = self.live_objects.saturating_sub(1);
                slot.object = None;
                slot.bytes = 0;
                slot.generation = slot.generation.wrapping_add(1);
                if let Ok(index) = u32::try_from(index) {
                    self.free_slots.push(index);
                }
            }
        }
        reclaimed
    }

    pub fn used_bytes(&self) -> usize {
        self.used_bytes
    }

    pub fn live_objects(&self) -> usize {
        self.live_objects
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskSource {
    Javascript,
    DomManipulation,
    Networking,
    Timer,
    UserInteraction,
    Media,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueuedJob {
    pub source: TaskSource,
    pub callback_id: u64,
    pub due_ms: u64,
    sequence: u64,
}

#[derive(Debug)]
pub struct AgentEventLoop {
    now_ms: u64,
    next_sequence: u64,
    max_tasks: usize,
    max_microtasks: usize,
    tasks: Vec<QueuedJob>,
    microtasks: VecDeque<QueuedJob>,
    truncated: bool,
}

impl AgentEventLoop {
    pub fn new(max_tasks: usize, max_microtasks: usize) -> Self {
        Self {
            now_ms: 0,
            next_sequence: 0,
            max_tasks,
            max_microtasks,
            tasks: Vec::new(),
            microtasks: VecDeque::new(),
            truncated: false,
        }
    }

    pub fn queue_task(
        &mut self,
        source: TaskSource,
        callback_id: u64,
        delay_ms: u64,
    ) -> Result<(), String> {
        if self.tasks.len() >= self.max_tasks {
            self.truncated = true;
            return Err("Runtime task budget exceeded".to_string());
        }
        let job = QueuedJob {
            source,
            callback_id,
            due_ms: self.now_ms.saturating_add(delay_ms.min(60_000)),
            sequence: self.take_sequence(),
        };
        self.tasks.push(job);
        Ok(())
    }

    pub fn queue_microtask(&mut self, callback_id: u64) -> Result<(), String> {
        if self.microtasks.len() >= self.max_microtasks {
            self.truncated = true;
            return Err("Runtime microtask budget exceeded".to_string());
        }
        let job = QueuedJob {
            source: TaskSource::Javascript,
            callback_id,
            due_ms: self.now_ms,
            sequence: self.take_sequence(),
        };
        self.microtasks.push_back(job);
        Ok(())
    }

    pub fn advance_time(&mut self, elapsed_ms: u64) {
        self.now_ms = self.now_ms.saturating_add(elapsed_ms);
    }

    pub fn pop_next_ready(&mut self) -> Option<QueuedJob> {
        if let Some(microtask) = self.microtasks.pop_front() {
            return Some(microtask);
        }
        let index = self
            .tasks
            .iter()
            .enumerate()
            .filter(|(_, task)| task.due_ms <= self.now_ms)
            .min_by_key(|(_, task)| (task.due_ms, task.sequence))
            .map(|(index, _)| index)?;
        Some(self.tasks.remove(index))
    }

    pub fn pending_len(&self) -> usize {
        self.tasks.len() + self.microtasks.len()
    }

    pub fn was_truncated(&self) -> bool {
        self.truncated
    }

    fn take_sequence(&mut self) -> u64 {
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.saturating_add(1);
        sequence
    }
}

#[derive(Debug)]
pub struct ExecutionBudget {
    remaining_instructions: u64,
    call_depth: usize,
    max_call_depth: usize,
}

impl ExecutionBudget {
    pub fn new(max_instructions: u64, max_call_depth: usize) -> Self {
        Self {
            remaining_instructions: max_instructions,
            call_depth: 0,
            max_call_depth,
        }
    }

    pub fn consume(&mut self, amount: u64) -> Result<(), String> {
        self.remaining_instructions = self
            .remaining_instructions
            .checked_sub(amount)
            .ok_or_else(|| "Runtime instruction budget exceeded".to_string())?;
        Ok(())
    }

    pub fn enter_call(&mut self) -> Result<(), String> {
        if self.call_depth >= self.max_call_depth {
            return Err("Runtime call-depth budget exceeded".to_string());
        }
        self.call_depth += 1;
        Ok(())
    }

    pub fn exit_call(&mut self) {
        self.call_depth = self.call_depth.saturating_sub(1);
    }

    pub fn remaining_instructions(&self) -> u64 {
        self.remaining_instructions
    }
}

#[derive(Debug)]
pub struct RuntimeRealm {
    pub id: u64,
    pub heap: BoundedHeap,
    pub global: HeapHandle,
    pub document: HeapHandle,
    pub event_loop: AgentEventLoop,
    pub execution: ExecutionBudget,
}

impl RuntimeRealm {
    pub fn new(id: u64, limits: RuntimeLimits) -> Result<Self, String> {
        let mut heap = BoundedHeap::new(limits.clone());
        let global = heap.allocate(HostObjectKind::Global)?;
        let document = heap.allocate(HostObjectKind::Document)?;
        heap.set_property(global, "document", RuntimeValue::Object(document))?;
        heap.set_property(global, "window", RuntimeValue::Object(global))?;
        Ok(Self {
            id,
            heap,
            global,
            document,
            event_loop: AgentEventLoop::new(limits.max_tasks, limits.max_microtasks),
            execution: ExecutionBudget::new(limits.max_instructions, limits.max_call_depth),
        })
    }

    pub fn collect_garbage(&mut self) -> usize {
        self.heap.collect_garbage(&[self.global, self.document])
    }
}

pub fn webidl_to_boolean(value: &RuntimeValue) -> bool {
    match value {
        RuntimeValue::Undefined | RuntimeValue::Null => false,
        RuntimeValue::Boolean(value) => *value,
        RuntimeValue::Number(value) => *value != 0.0 && !value.is_nan(),
        RuntimeValue::String(value) => !value.is_empty(),
        RuntimeValue::Object(_) => true,
    }
}

pub fn webidl_to_dom_string(value: &RuntimeValue) -> Result<String, String> {
    let output = match value {
        RuntimeValue::Undefined => "undefined".to_string(),
        RuntimeValue::Null => "null".to_string(),
        RuntimeValue::Boolean(value) => value.to_string(),
        RuntimeValue::Number(value) => value.to_string(),
        RuntimeValue::String(value) => value.clone(),
        RuntimeValue::Object(_) => "[object Object]".to_string(),
    };
    if output.len() > MAX_DOM_STRING_BYTES {
        return Err("DOMString exceeds 1 MB".to_string());
    }
    Ok(output)
}

fn property_cost(name: &str, value: &RuntimeValue) -> usize {
    PROPERTY_BASE_BYTES
        .saturating_add(name.len())
        .saturating_add(match value {
            RuntimeValue::String(text) => text.len(),
            _ => std::mem::size_of::<RuntimeValue>(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heap_reclaims_unreachable_objects_and_rejects_stale_handles() {
        let mut heap = BoundedHeap::new(RuntimeLimits::default());
        let root = heap.allocate(HostObjectKind::Global).unwrap();
        let child = heap.allocate(HostObjectKind::Element).unwrap();
        let garbage = heap.allocate(HostObjectKind::Ordinary).unwrap();
        heap.set_property(root, "child", RuntimeValue::Object(child))
            .unwrap();
        let before = heap.used_bytes();
        assert!(heap.collect_garbage(&[root]) > 0);
        assert!(heap.used_bytes() < before);
        assert!(heap.get(root).is_some());
        assert!(heap.get(child).is_some());
        assert!(heap.get(garbage).is_none());

        let reused = heap.allocate(HostObjectKind::Promise).unwrap();
        assert_ne!(garbage, reused);
        assert!(heap.get(garbage).is_none());
    }

    #[test]
    fn heap_enforces_property_and_byte_limits() {
        let limits = RuntimeLimits {
            max_heap_bytes: 512,
            max_properties_per_object: 1,
            ..RuntimeLimits::default()
        };
        let mut heap = BoundedHeap::new(limits);
        let object = heap.allocate(HostObjectKind::Ordinary).unwrap();
        heap.set_property(object, "name", RuntimeValue::String("safe".into()))
            .unwrap();
        assert!(heap
            .set_property(object, "second", RuntimeValue::Boolean(true))
            .is_err());
        assert!(heap
            .set_property(object, "name", RuntimeValue::String("x".repeat(1_024)))
            .is_err());
    }

    #[test]
    fn heap_reclaims_an_unreachable_reference_cycle() {
        let mut heap = BoundedHeap::new(RuntimeLimits::default());
        let root = heap.allocate(HostObjectKind::Global).unwrap();
        let left = heap.allocate(HostObjectKind::Ordinary).unwrap();
        let right = heap.allocate(HostObjectKind::Ordinary).unwrap();
        heap.set_property(left, "right", RuntimeValue::Object(right))
            .unwrap();
        heap.set_property(right, "left", RuntimeValue::Object(left))
            .unwrap();

        let before = heap.live_objects();
        assert!(heap.collect_garbage(&[root]) > 0);
        assert_eq!(before - heap.live_objects(), 2);
        assert!(heap.get(left).is_none());
        assert!(heap.get(right).is_none());
    }

    #[test]
    fn event_loop_runs_microtasks_before_ready_tasks() {
        let mut event_loop = AgentEventLoop::new(4, 4);
        event_loop.queue_task(TaskSource::Networking, 1, 0).unwrap();
        event_loop.queue_microtask(2).unwrap();
        event_loop.queue_task(TaskSource::Timer, 3, 10).unwrap();
        assert_eq!(event_loop.pop_next_ready().unwrap().callback_id, 2);
        assert_eq!(event_loop.pop_next_ready().unwrap().callback_id, 1);
        assert!(event_loop.pop_next_ready().is_none());
        event_loop.advance_time(10);
        assert_eq!(event_loop.pop_next_ready().unwrap().callback_id, 3);
    }

    #[test]
    fn event_loop_limits_fail_closed_and_record_truncation() {
        let mut event_loop = AgentEventLoop::new(1, 1);
        event_loop
            .queue_task(TaskSource::UserInteraction, 1, 0)
            .unwrap();
        assert!(event_loop
            .queue_task(TaskSource::UserInteraction, 2, 0)
            .is_err());
        event_loop.queue_microtask(3).unwrap();
        assert!(event_loop.queue_microtask(4).is_err());
        assert!(event_loop.was_truncated());
        assert_eq!(event_loop.pending_len(), 2);
    }

    #[test]
    fn realm_has_bounded_global_document_and_execution_budget() {
        let mut realm = RuntimeRealm::new(7, RuntimeLimits::default()).unwrap();
        assert_eq!(realm.id, 7);
        assert_eq!(realm.heap.live_objects(), 2);
        assert_eq!(
            realm.heap.get(realm.global).unwrap().property("document"),
            Some(&RuntimeValue::Object(realm.document))
        );
        realm.execution.consume(10).unwrap();
        realm.execution.enter_call().unwrap();
        realm.execution.exit_call();
        assert_eq!(realm.execution.remaining_instructions(), 999_990);
        assert_eq!(realm.collect_garbage(), 0);
    }

    #[test]
    fn execution_and_webidl_conversions_are_bounded() {
        let mut budget = ExecutionBudget::new(2, 1);
        budget.consume(2).unwrap();
        assert!(budget.consume(1).is_err());
        budget.enter_call().unwrap();
        assert!(budget.enter_call().is_err());
        budget.exit_call();

        assert!(!webidl_to_boolean(&RuntimeValue::Number(f64::NAN)));
        assert!(!webidl_to_boolean(&RuntimeValue::String(String::new())));
        assert!(webidl_to_boolean(&RuntimeValue::String("0".into())));
        assert_eq!(webidl_to_dom_string(&RuntimeValue::Null).unwrap(), "null");
        assert!(
            webidl_to_dom_string(&RuntimeValue::String("x".repeat(MAX_DOM_STRING_BYTES + 1)))
                .is_err()
        );
    }
}
