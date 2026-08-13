//! Bounded ECMAScript Promise records and reaction jobs.
//!
//! The runtime stores callback identifiers instead of Rust closures. This keeps
//! host authority outside the language heap and lets the interpreter invoke a
//! reaction under its own instruction and call-depth budgets.

use std::collections::BTreeMap;

use crate::runtime_core::{AgentEventLoop, RuntimeValue, TaskSource};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PromiseId(u64);

#[derive(Debug, Clone, PartialEq)]
pub enum PromiseState {
    Pending,
    Fulfilled(RuntimeValue),
    Rejected(RuntimeValue),
}

#[derive(Debug, Clone)]
struct PromiseReaction {
    on_fulfilled: Option<u64>,
    on_rejected: Option<u64>,
    result: PromiseId,
}

#[derive(Debug, Clone)]
struct PromiseRecord {
    state: PromiseState,
    reactions: Vec<PromiseReaction>,
}

#[derive(Debug, Clone)]
struct QueuedReaction {
    handler_id: Option<u64>,
    argument: RuntimeValue,
    result: PromiseId,
    rejected_input: bool,
}

#[derive(Debug)]
pub struct PromiseRuntime {
    max_promises: usize,
    next_promise_id: u64,
    next_job_id: u64,
    records: BTreeMap<PromiseId, PromiseRecord>,
    jobs: BTreeMap<u64, QueuedReaction>,
    event_loop: AgentEventLoop,
}

impl PromiseRuntime {
    pub fn new(max_promises: usize, max_microtasks: usize) -> Self {
        Self {
            max_promises,
            next_promise_id: 1,
            next_job_id: 1,
            records: BTreeMap::new(),
            jobs: BTreeMap::new(),
            event_loop: AgentEventLoop::new(0, max_microtasks),
        }
    }

    pub fn create(&mut self) -> Result<PromiseId, String> {
        if self.records.len() >= self.max_promises {
            return Err("Promise record budget exceeded".to_string());
        }
        let id = PromiseId(self.next_promise_id);
        self.next_promise_id = self
            .next_promise_id
            .checked_add(1)
            .ok_or_else(|| "Promise identifier space exhausted".to_string())?;
        self.records.insert(
            id,
            PromiseRecord {
                state: PromiseState::Pending,
                reactions: Vec::new(),
            },
        );
        Ok(id)
    }

    pub fn state(&self, id: PromiseId) -> Option<&PromiseState> {
        self.records.get(&id).map(|record| &record.state)
    }

    pub fn pending_jobs(&self) -> usize {
        self.jobs.len()
    }

    pub fn then(
        &mut self,
        promise: PromiseId,
        on_fulfilled: Option<u64>,
        on_rejected: Option<u64>,
    ) -> Result<PromiseId, String> {
        let state = self
            .state(promise)
            .cloned()
            .ok_or_else(|| "Unknown Promise record".to_string())?;
        let result = self.create()?;
        let reaction = PromiseReaction {
            on_fulfilled,
            on_rejected,
            result,
        };

        let enqueue_result = match state {
            PromiseState::Pending => {
                self.records
                    .get_mut(&promise)
                    .expect("Promise record was validated")
                    .reactions
                    .push(reaction);
                Ok(())
            }
            PromiseState::Fulfilled(value) => self.enqueue_reaction(reaction, value, false),
            PromiseState::Rejected(reason) => self.enqueue_reaction(reaction, reason, true),
        };
        if let Err(error) = enqueue_result {
            self.records.remove(&result);
            return Err(error);
        }
        Ok(result)
    }

    pub fn resolve(&mut self, promise: PromiseId, value: RuntimeValue) -> Result<(), String> {
        self.settle(promise, PromiseState::Fulfilled(value))
    }

    pub fn reject(&mut self, promise: PromiseId, reason: RuntimeValue) -> Result<(), String> {
        self.settle(promise, PromiseState::Rejected(reason))
    }

    pub fn drain_jobs<F>(&mut self, max_jobs: usize, mut invoke: F) -> Result<usize, String>
    where
        F: FnMut(u64, RuntimeValue) -> Result<RuntimeValue, RuntimeValue>,
    {
        let mut processed = 0;
        while processed < max_jobs {
            let Some(job) = self.event_loop.pop_next_ready() else {
                break;
            };
            if job.source != TaskSource::Javascript {
                return Err("Promise runtime received a non-JavaScript job".to_string());
            }
            let reaction = self
                .jobs
                .remove(&job.callback_id)
                .ok_or_else(|| "Promise reaction job is missing".to_string())?;
            let outcome = match reaction.handler_id {
                Some(handler_id) => invoke(handler_id, reaction.argument),
                None if reaction.rejected_input => Err(reaction.argument),
                None => Ok(reaction.argument),
            };
            match outcome {
                Ok(value) => self.resolve(reaction.result, value)?,
                Err(reason) => self.reject(reaction.result, reason)?,
            }
            processed += 1;
        }
        if self.pending_jobs() > 0 {
            return Err("Promise job execution budget exceeded".to_string());
        }
        Ok(processed)
    }

    fn settle(&mut self, promise: PromiseId, state: PromiseState) -> Result<(), String> {
        if matches!(state, PromiseState::Pending) {
            return Err("Cannot settle a Promise as pending".to_string());
        }
        let record = self
            .records
            .get_mut(&promise)
            .ok_or_else(|| "Unknown Promise record".to_string())?;
        if !matches!(record.state, PromiseState::Pending) {
            return Ok(());
        }
        let reactions = std::mem::take(&mut record.reactions);
        record.state = state.clone();
        let (argument, rejected_input) = match state {
            PromiseState::Fulfilled(value) => (value, false),
            PromiseState::Rejected(reason) => (reason, true),
            PromiseState::Pending => unreachable!(),
        };

        let mut first_error = None;
        for reaction in reactions {
            let result = reaction.result;
            if let Err(error) = self.enqueue_reaction(reaction, argument.clone(), rejected_input) {
                if let Some(record) = self.records.get_mut(&result) {
                    record.state = PromiseState::Rejected(RuntimeValue::String(error.clone()));
                }
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    fn enqueue_reaction(
        &mut self,
        reaction: PromiseReaction,
        argument: RuntimeValue,
        rejected_input: bool,
    ) -> Result<(), String> {
        let job_id = self.next_job_id;
        let next_job_id = job_id
            .checked_add(1)
            .ok_or_else(|| "Promise job identifier space exhausted".to_string())?;
        self.event_loop.queue_microtask(job_id)?;
        self.next_job_id = next_job_id;
        self.jobs.insert(
            job_id,
            QueuedReaction {
                handler_id: if rejected_input {
                    reaction.on_rejected
                } else {
                    reaction.on_fulfilled
                },
                argument,
                result: reaction.result,
                rejected_input,
            },
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reactions_are_asynchronous_and_chain_fulfillment() {
        let mut runtime = PromiseRuntime::new(8, 8);
        let source = runtime.create().unwrap();
        let chained = runtime.then(source, Some(42), None).unwrap();
        runtime.resolve(source, RuntimeValue::Number(7.0)).unwrap();
        assert_eq!(runtime.state(chained), Some(&PromiseState::Pending));

        let mut calls = Vec::new();
        let processed = runtime
            .drain_jobs(8, |handler, value| {
                calls.push((handler, value.clone()));
                Ok(RuntimeValue::Number(8.0))
            })
            .unwrap();
        assert_eq!(processed, 1);
        assert_eq!(calls, vec![(42, RuntimeValue::Number(7.0))]);
        assert_eq!(
            runtime.state(chained),
            Some(&PromiseState::Fulfilled(RuntimeValue::Number(8.0)))
        );
    }

    #[test]
    fn missing_handlers_propagate_fulfillment_and_rejection() {
        let mut runtime = PromiseRuntime::new(8, 8);
        let fulfilled = runtime.create().unwrap();
        let fulfilled_child = runtime.then(fulfilled, None, None).unwrap();
        runtime
            .resolve(fulfilled, RuntimeValue::String("ok".into()))
            .unwrap();
        runtime.drain_jobs(8, |_, _| unreachable!()).unwrap();
        assert_eq!(
            runtime.state(fulfilled_child),
            Some(&PromiseState::Fulfilled(RuntimeValue::String("ok".into())))
        );

        let rejected = runtime.create().unwrap();
        let rejected_child = runtime.then(rejected, None, None).unwrap();
        runtime
            .reject(rejected, RuntimeValue::String("failure".into()))
            .unwrap();
        runtime.drain_jobs(8, |_, _| unreachable!()).unwrap();
        assert_eq!(
            runtime.state(rejected_child),
            Some(&PromiseState::Rejected(RuntimeValue::String(
                "failure".into()
            )))
        );
    }

    #[test]
    fn thrown_handler_value_rejects_the_chained_promise() {
        let mut runtime = PromiseRuntime::new(4, 4);
        let source = runtime.create().unwrap();
        let chained = runtime.then(source, Some(1), None).unwrap();
        runtime.resolve(source, RuntimeValue::Undefined).unwrap();
        runtime
            .drain_jobs(4, |_, _| Err(RuntimeValue::String("boom".into())))
            .unwrap();
        assert_eq!(
            runtime.state(chained),
            Some(&PromiseState::Rejected(RuntimeValue::String("boom".into())))
        );
    }

    #[test]
    fn promise_and_job_budgets_fail_closed() {
        let mut promises = PromiseRuntime::new(1, 1);
        promises.create().unwrap();
        assert!(promises.create().is_err());

        let mut jobs = PromiseRuntime::new(8, 1);
        let first = jobs.create().unwrap();
        let second = jobs.create().unwrap();
        jobs.then(first, Some(1), None).unwrap();
        jobs.then(second, Some(2), None).unwrap();
        jobs.resolve(first, RuntimeValue::Undefined).unwrap();
        assert!(jobs.resolve(second, RuntimeValue::Undefined).is_err());
        assert!(jobs.event_loop.was_truncated());
    }

    #[test]
    fn drain_budget_stops_an_unbounded_reaction_chain() {
        let mut runtime = PromiseRuntime::new(8, 8);
        let first = runtime.create().unwrap();
        let second = runtime.then(first, Some(1), None).unwrap();
        let third = runtime.then(second, Some(2), None).unwrap();
        runtime.resolve(first, RuntimeValue::Undefined).unwrap();
        assert!(runtime
            .drain_jobs(1, |_, _| Ok(RuntimeValue::Undefined))
            .is_err());
        assert_eq!(runtime.pending_jobs(), 1);
        assert_eq!(runtime.state(third), Some(&PromiseState::Pending));
    }
}
