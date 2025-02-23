@0x93e2c78a503bc43a;

struct EventId {
  p1 @0 : UInt64;
  p2 @1 : UInt64;
}

enum EventType {
  pmuCycles @0;
  pmuInstructions @1;
  pmuLLCReferences @2;
  pmuLLCMisses @3;
  pmuBranchInstructions @4;
  pmuBranchMisses @5;
  pmuStalledCyclesFrontend @6;
  pmuStalledCyclesBackend @7;
  pmuCustom @8;
  osCpuClock @9;
  osCpuMigrations @10;
  osPageFaults @11;
  osContextSwitches @12;
  osTotalTime @13;
  osUserTime @14;
  osSystemTime @15;
  rooflineBytesLoad @16;
  rooflineBytesStore @17;
  rooflineScalarIntOps @18;
  rooflineScalarFloatOps @19;
  rooflineScalarDoubleOps @20;
  rooflineVectorIntOps @21;
  rooflineVectorFloatOps @22;
  rooflineVectorDoubleOps @23;
  rooflineLoopStart @24;
  rooflineLoopEnd @25;
}

struct Location {
  functionName @0 : EventId;
  filename @1 : EventId;
  line @2 : UInt32;
}

struct CallFrame {
  union {
    location @0 :Location;
    ip @1 :UInt64;
  }
}

struct Metadata {
  key @0 : EventId;
  union {
    string @1 : EventId;
    integer @2 : UInt64;
  }
}

struct Event {
  uniqueId @0 : EventId;
  parentId @1 : EventId;
  correlationId @2 : EventId;
  ty @3 : EventType;
  processId @4 : UInt32;
  threadId @5 : UInt32;
  timeEnabled @6 : UInt64;
  timeRunning @7 : UInt64;
  timestamp @8 : UInt64;
  value @9 : UInt64;
  ip @10 : UInt64;
  callstack @11 : List(CallFrame);
  metadata @12 : List(Metadata);
}
