include!(concat!(env!("OUT_DIR"), "/schema/event_capnp.rs"));

impl<'a> event::Builder<'a> {
    pub fn set_event(&mut self, event: &crate::Event) {
        {
            let mut unique_id = self.reborrow().init_unique_id();
            unique_id.set_p1(0);
            unique_id.set_p2(event.unique_id);
        }
        {
            let mut parent_id = self.reborrow().init_parent_id();
            parent_id.set_p1(0);
            parent_id.set_p2(event.parent_id);
        }
        {
            let mut correlation_id = self.reborrow().init_correlation_id();
            correlation_id.set_p1(0);
            correlation_id.set_p2(event.correlation_id);
        }

        self.reborrow().set_ty(event.ty.into());
        self.reborrow().set_process_id(event.process_id);
        self.reborrow().set_thread_id(event.thread_id);
        self.reborrow().set_time_enabled(event.time_enabled);
        self.reborrow().set_time_running(event.time_running);
        self.reborrow().set_timestamp(event.timestamp);
        self.reborrow().set_value(event.value);
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
