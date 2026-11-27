- **An ACP seat task records the context, cost estimate and model its
  adapter reported.** The context occupancy of the latest `usage_update`
  rides as `context_used` and `context_size`, and Claude's session cost as
  `cost_estimate`, the adapter's own unbilled estimate with its currency: it
  never becomes `cost_usd` or reaches a cost ceiling, an update without a
  cost clears it, and a cost whose currency is not three capital letters, or
  whose amount is negative or not a finite number, is dropped. The model
  Claude reports having answered with is the task's `model_reported` (source
  `adapter_usage_update`) and the receipt's responder, beside the configured
  model; under an exact selection, a reported model that is demonstrably
  another one (another model line, version or release date) refuses the
  answer with NIKA-1805, as a model moved during the turn does, and a
  spelling Nika cannot place is only recorded.
