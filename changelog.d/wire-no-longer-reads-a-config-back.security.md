- **`nika wire` no longer reads a config back after writing it.** Since
  0.123.0 it did, and when the file did not hold what it wrote it put
  the old text back (or removed the new file) and failed with exit 3,
  "the file read back is not what nika wire wrote". That repair read and
  wrote the config by its name a second time, after the check, where a
  link put there in the meantime would have received it, so it is
  removed with the read-back. A config another program changes right
  after the write is no longer reported or restored; the next `nika
  wire` finds it and updates it again.
