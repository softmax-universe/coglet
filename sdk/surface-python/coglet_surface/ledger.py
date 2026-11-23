"""Single-owner SQLite receipt ledger (local filesystem, not network storage)."""
import fcntl
import os
import sqlite3
import stat

from .errors import ProtocolError


class Ledger:
    def __init__(self, path, identity, create):
        self.fd = None
        self.db = None
        try:
            flags = os.O_RDWR | os.O_NOFOLLOW
            if create:
                flags |= os.O_CREAT | os.O_EXCL
            self.fd = os.open(path, flags, 0o600)
            info = os.fstat(self.fd)
            if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) != 0o600:
                raise ProtocolError('ledger must be an owned private regular file')
            try:
                fcntl.flock(self.fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError as exc:
                raise ProtocolError('ledger in use by another surface') from exc
            self.db = sqlite3.connect(path)
            if not create:
                retained = self.db.execute("SELECT value FROM meta WHERE key='identity'").fetchone()
                if not retained or retained[0] != identity:
                    raise ProtocolError('lost ledger or identity mismatch; reconcile and rotate identity')
                required = {'meta', 'actions'}
                tables = {r[0] for r in self.db.execute("SELECT name FROM sqlite_master WHERE type='table'")}
                if not required <= tables:
                    raise ProtocolError('incomplete ledger; reconcile and rotate identity')
            self.db.execute('PRAGMA journal_mode=WAL')
            self.db.execute('PRAGMA synchronous=FULL')
            if create:
                with self.db:
                    self.db.execute('CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT)')
                    self.db.execute('CREATE TABLE actions (coglet TEXT, id TEXT, envelope BLOB, '
                                    'receipt BLOB, outcome BLOB, PRIMARY KEY (coglet,id))')
                    self.db.execute("INSERT INTO meta VALUES ('identity', ?)", (identity,))
        except BaseException as exc:
            self.close()
            if isinstance(exc, FileExistsError):
                raise ProtocolError('ledger exists; creation requires a new identity and fresh file') from exc
            if isinstance(exc, (OSError, sqlite3.DatabaseError)):
                raise ProtocolError('lost, unsafe or corrupt ledger; reconcile and rotate identity') from exc
            raise

    def close(self):
        try:
            if self.db is not None:
                self.db.close()
        finally:
            if self.fd is not None:
                os.close(self.fd)
                self.fd = None
