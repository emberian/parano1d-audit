# agent-19: Attack RPC admission paths

(A final message from the kimi session wire; status may be completed OR died mid-lane)

The PoC will run the real loopback JSON-RPC server and track allocations only on the handler’s blocking query thread. This isolates the pagination heap from HTTP, database, wallet-index, and startup allocations.
