use crate::{
    event::{CallFrame, Location},
    Event,
};

use smallvec::smallvec;

include!(concat!(env!("OUT_DIR"), "/schema/event_capnp.rs"));

impl event::Builder<'_> {
    pub fn set_event(&mut self, event: &crate::Event) {
        {
            let mut unique_id = self.reborrow().init_unique_id();
            unique_id.set_p1((event.unique_id >> 64) as u64);
            unique_id.set_p2(event.unique_id as u64);
        }
        {
            let mut parent_id = self.reborrow().init_parent_id();
            parent_id.set_p1((event.parent_id >> 64) as u64);
            parent_id.set_p2(event.parent_id as u64);
        }
        {
            let mut correlation_id = self.reborrow().init_correlation_id();
            correlation_id.set_p1((event.correlation_id >> 64) as u64);
            correlation_id.set_p2(event.correlation_id as u64);
        }

        self.reborrow().set_ty(event.ty.into());
        self.reborrow().set_process_id(event.process_id);
        self.reborrow().set_thread_id(event.thread_id);
        self.reborrow().set_time_enabled(event.time_enabled);
        self.reborrow().set_time_running(event.time_running);
        self.reborrow().set_timestamp(event.timestamp);
        self.reborrow().set_value(event.value);

        if !event.callstack.is_empty() {
            let mut root = self.reborrow().init_callstack(event.callstack.len() as u32);
            for (id, frame) in event.callstack.iter().enumerate() {
                let mut capnp_frame = root.reborrow().get(id as u32);
                match frame {
                    CallFrame::IP(ip) => {
                        capnp_frame.set_ip(*ip);
                    }
                    CallFrame::Location(loc) => {
                        let mut capnp_loc = capnp_frame.reborrow().init_location();

                        let mut filename_id = capnp_loc.reborrow().init_filename();
                        filename_id.set_p1((loc.file_name >> 64) as u64);
                        filename_id.set_p2(loc.file_name as u64);

                        let mut func_name_id = capnp_loc.reborrow().init_function_name();
                        func_name_id.set_p1((loc.function_name >> 64) as u64);
                        func_name_id.set_p2(loc.function_name as u64);

                        capnp_loc.set_line(loc.line);
                    }
                }
            }
        }
    }
}

impl From<event::Reader<'_>> for Event {
    fn from(val: event::Reader<'_>) -> Self {
        let unique_id = ((val.get_unique_id().expect("unique_id").get_p1() as u128) << 64)
            | ((val.get_unique_id().expect("unique_id").get_p2()) as u128);
        let parent_id = ((val.get_parent_id().expect("parent_id").get_p1() as u128) << 64)
            | ((val.get_parent_id().expect("parent_id").get_p2()) as u128);
        let correlation_id = ((val.get_correlation_id().expect("correlation_id").get_p1() as u128)
            << 64)
            | ((val.get_correlation_id().expect("correlation_id").get_p2()) as u128);

        let mut callstack = smallvec![];

        let callstack_list = val.get_callstack().expect("callstack");

        for frame in callstack_list {
            match frame.which() {
                Ok(call_frame::Location(Ok(loc))) => {
                    let location = Location {
                        file_name: ((loc.get_filename().expect("filename").get_p1() as u128) << 64)
                            | (loc.get_filename().expect("filename").get_p2() as u128),
                        function_name: ((loc.get_function_name().expect("function_name").get_p1()
                            as u128)
                            << 64)
                            | (loc.get_function_name().expect("function_name").get_p2() as u128),
                        line: loc.get_line(),
                    };
                    callstack.push(CallFrame::Location(location));
                }
                Ok(call_frame::Ip(ip)) => {
                    callstack.push(CallFrame::IP(ip));
                }
                _ => unimplemented!(),
            }
        }

        Event {
            unique_id,
            correlation_id,
            parent_id,
            ty: val.get_ty().expect("ty").into(),
            thread_id: val.get_thread_id(),
            process_id: val.get_process_id(),
            time_enabled: val.get_time_enabled(),
            time_running: val.get_time_running(),
            value: val.get_value(),
            timestamp: val.get_timestamp(),
            callstack,
        }
    }
}

impl From<crate::EventType> for EventType {
    fn from(value: crate::EventType) -> Self {
        match value {
            crate::EventType::PmuCycles => EventType::PmuCycles,
            crate::EventType::PmuInstructions => EventType::PmuInstructions,
            crate::EventType::PmuLlcReferences => EventType::PmuLLCReferences,
            crate::EventType::PmuLlcMisses => EventType::PmuLLCMisses,
            crate::EventType::PmuBranchInstructions => EventType::PmuBranchInstructions,
            crate::EventType::PmuBranchMisses => EventType::PmuBranchMisses,
            crate::EventType::PmuStalledCyclesFrontend => EventType::PmuStalledCyclesFrontend,
            crate::EventType::PmuStalledCyclesBackend => EventType::PmuStalledCyclesBackend,
            crate::EventType::PmuCustom => EventType::PmuCustom,
            crate::EventType::OsCpuClock => EventType::OsCpuClock,
            crate::EventType::OsCpuMigrations => EventType::OsCpuMigrations,
            crate::EventType::OsPageFaults => EventType::OsPageFaults,
            crate::EventType::OsContextSwitches => EventType::OsContextSwitches,
            crate::EventType::OsTotalTime => EventType::OsTotalTime,
            crate::EventType::OsUserTime => EventType::OsUserTime,
            crate::EventType::OsSystemTime => EventType::OsSystemTime,
            crate::EventType::RooflineBytesLoad => EventType::RooflineBytesLoad,
            crate::EventType::RooflineBytesStore => EventType::RooflineBytesStore,
            crate::EventType::RooflineScalarIntOps => EventType::RooflineScalarIntOps,
            crate::EventType::RooflineScalarFloatOps => EventType::RooflineScalarFloatOps,
            crate::EventType::RooflineScalarDoubleOps => EventType::RooflineScalarDoubleOps,
            crate::EventType::RooflineVectorIntOps => EventType::RooflineVectorIntOps,
            crate::EventType::RooflineVectorFloatOps => EventType::RooflineVectorFloatOps,
            crate::EventType::RooflineVectorDoubleOps => EventType::RooflineVectorDoubleOps,
            crate::EventType::RooflineLoopStart => EventType::RooflineLoopStart,
            crate::EventType::RooflineLoopEnd => EventType::RooflineLoopEnd,
        }
    }
}

impl From<EventType> for crate::EventType {
    fn from(value: EventType) -> Self {
        match value {
            EventType::PmuCycles => crate::EventType::PmuCycles,
            EventType::PmuInstructions => crate::EventType::PmuInstructions,
            EventType::PmuLLCReferences => crate::EventType::PmuLlcReferences,
            EventType::PmuLLCMisses => crate::EventType::PmuLlcMisses,
            EventType::PmuBranchInstructions => crate::EventType::PmuBranchInstructions,
            EventType::PmuBranchMisses => crate::EventType::PmuBranchMisses,
            EventType::PmuStalledCyclesFrontend => crate::EventType::PmuStalledCyclesFrontend,
            EventType::PmuStalledCyclesBackend => crate::EventType::PmuStalledCyclesBackend,
            EventType::PmuCustom => crate::EventType::PmuCustom,
            EventType::OsCpuClock => crate::EventType::OsCpuClock,
            EventType::OsCpuMigrations => crate::EventType::OsCpuMigrations,
            EventType::OsPageFaults => crate::EventType::OsPageFaults,
            EventType::OsContextSwitches => crate::EventType::OsContextSwitches,
            EventType::OsTotalTime => crate::EventType::OsTotalTime,
            EventType::OsUserTime => crate::EventType::OsUserTime,
            EventType::OsSystemTime => crate::EventType::OsSystemTime,
            EventType::RooflineBytesLoad => crate::EventType::RooflineBytesLoad,
            EventType::RooflineBytesStore => crate::EventType::RooflineBytesStore,
            EventType::RooflineScalarIntOps => crate::EventType::RooflineScalarIntOps,
            EventType::RooflineScalarFloatOps => crate::EventType::RooflineScalarFloatOps,
            EventType::RooflineScalarDoubleOps => crate::EventType::RooflineScalarDoubleOps,
            EventType::RooflineVectorIntOps => crate::EventType::RooflineVectorIntOps,
            EventType::RooflineVectorFloatOps => crate::EventType::RooflineVectorFloatOps,
            EventType::RooflineVectorDoubleOps => crate::EventType::RooflineVectorDoubleOps,
            EventType::RooflineLoopStart => crate::EventType::RooflineLoopStart,
            EventType::RooflineLoopEnd => crate::EventType::RooflineLoopEnd,
        }
    }
}
