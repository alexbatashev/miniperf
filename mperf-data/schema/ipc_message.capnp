@0xc15f34a2f6646fe9;

using Event = import "event.capnp".Event;

struct IpcString {
  key @0 :UInt64;
  value @1 :Text;
}

struct IpcMessage {
  union {
    event @0 :Event;
    string @1 :IpcString;
  }
}

interface IpcService {
  post @0 (message: IpcMessage) -> ();
}
