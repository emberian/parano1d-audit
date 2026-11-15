# agent-26: Attack DEX market economics

(A final message from the kimi session wire; status may be completed OR died mid-lane)

The curve’s tiny-buy assumption was also wrong in the oracle: at the initial price, 1 μNOID buys 3 token base units, so those are valid nonzero fills. I’ve changed the matrix to validate those fills and reserve rejection accounting for actual zero-output cases only.
