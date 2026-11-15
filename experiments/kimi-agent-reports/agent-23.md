# agent-23: Audit wallet secret safety

(A final message from the kimi session wire; status may be completed OR died mid-lane)

The stale-wallet repro passed: after key B replaced key A on disk, the loaded A wallet still produced a valid A-authorized intent, then wrote A’s address into B’s history. I’m running the complete four-test Rust suite and seven-test source/CLI suite once more before summarizing severity.
