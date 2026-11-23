// Familiar's engine as a C interface. Everything is one call: a method name and its arguments
// as JSON in, a JSON reply out. See crates/bridge/src/lib.rs for the methods.
#ifndef FAMILIAR_BRIDGE_H
#define FAMILIAR_BRIDGE_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// Called (from any thread) when a session has something for `session.poll` to collect.
typedef void (*familiar_wake_fn)(uint64_t session);

// Call once, on the thread that will make every other call (the main thread).
void familiar_init(familiar_wake_fn wake);

// Runs a method. The reply is {"ok": ...} or {"error": "..."}; free it with familiar_free.
// Only the methods documented as thread-safe may be called off the main thread.
char *familiar_call(const char *method, const char *args_json);

void familiar_free(char *reply);

#ifdef __cplusplus
}
#endif

#endif
