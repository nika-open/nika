- **The `schema:` of an ACP seat task and its repair judge the turn's last
  message.** A tool call always starts a new message, and so does a chunk
  whose `messageId` differs from the current message's or that carries an id
  after chunks without one, so narration before a tool call is no longer
  parsed as the answer. A failure keeps every message of the turn; a success
  of several messages keeps the whole text in the trace (`seat_transcript`,
  its first 64 KiB, with the run's secrets masked). One turn may hold 4 MiB
  of text.
