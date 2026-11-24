- **`nika explain NIKA-SEC-009` prints a fix, and it starts with the legs.** The
  page described the trifecta but printed no fix, and the finding's own fix
  names only the human gate. It now teaches dropping a leg the workflow does
  not need first, with the grant that arms each leg (`exec` arms both
  untrusted ingress and external egress), since a boundary missing a leg is
  safe by construction. When all three legs are needed, it gives both gate
  placements the check accepts: after the reads, where the question can show
  the content that would leave, or upstream of every entry task, where the
  person consents to the run before the untrusted content exists. Either way
  each effect after the gate tests its answer, since NIKA-SEC-014 refuses an
  effect it can prove runs on a « no » to a confirm gate. The MCP `nika_explain` tool prints
  the same fix.
