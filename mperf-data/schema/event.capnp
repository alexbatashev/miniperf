@0x93e2c78a503bc43a;

struct EventId {
  p1 @0 : UInt64;
  p2 @0 : UInt64;
}

enum EventType {
  PMU,
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
}
