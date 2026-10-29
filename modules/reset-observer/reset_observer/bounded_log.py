"""Keep one bounded diagnostic log; never rotate or copy valid ledger data."""
from contextlib import contextmanager
import io
from pathlib import Path
import sys
import threading

from .health import LOG_LIMIT_BYTES


class BoundedLog(io.TextIOBase):
    def __init__(self, path, limit=LOG_LIMIT_BYTES):
        self.limit = limit
        self.lock = threading.RLock()
        self.stream = Path(path).open("a+b")
        self._compact()

    @property
    def encoding(self):
        return "utf-8"

    def _compact(self):
        self.stream.seek(0, 2)
        if self.stream.tell() <= self.limit:
            return
        self.stream.seek(-self.limit // 2, 2)
        tail = self.stream.read()
        boundary = tail.find(b"\n")
        if boundary >= 0:
            tail = tail[boundary + 1:]
        tail = tail.decode("utf-8", errors="ignore").encode("utf-8")
        self.stream.seek(0)
        self.stream.truncate()
        self.stream.write(tail)
        self.stream.flush()

    def write(self, text):
        value = text.encode("utf-8")
        if len(value) > self.limit:
            value = value[-self.limit // 2:].decode("utf-8", errors="ignore").encode("utf-8")
        with self.lock:
            self.stream.write(value)
            self.stream.flush()
            self._compact()
        return len(text)

    def flush(self):
        with self.lock:
            if not self.stream.closed:
                self.stream.flush()

    def close(self):
        with self.lock:
            if not self.closed:
                super().close()
                self.stream.close()


def compact_log(path):
    with BoundedLog(path):
        pass


@contextmanager
def managed_log(db, enabled):
    if not enabled:
        yield
        return
    original = sys.stdout, sys.stderr
    with BoundedLog(str(Path(db).resolve()) + ".log") as log:
        try:
            sys.stdout = sys.stderr = log
            yield
        finally:
            sys.stdout, sys.stderr = original
