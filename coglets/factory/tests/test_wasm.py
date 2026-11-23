"""Supplemental real Wasm -> DB buffer -> SQLite proof, NOT runtime conformance.

Uses an in-process test transport, no production admission/journal/recovery. Keep
0.1 admission closed until the real engines implement the 0.2 extension.
"""
import pathlib
import sqlite3
import sys
import unittest
import wasmtime

ROOT = pathlib.Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from conformance.wire import encode, decode

MODULE = ROOT / "target/factory/wasm32-wasip1/release/coglet_factory.wasm"
ID = "00000000000000000000000001"

class FactoryWasm(unittest.TestCase):
    def run_guest(self, db_result=0):
        engine = wasmtime.Engine()
        store = wasmtime.Store(engine)
        store.set_wasi(wasmtime.WasiConfig())
        module = wasmtime.Module.from_file(engine, str(MODULE))
        self.assertFalse({"clock_time_get", "random_get", "poll_oneoff"}.intersection(i.name for i in module.imports))
        db_import = [i for i in module.imports if i.module == "coglet_v0" and i.name == "db"]
        self.assertEqual(len(db_import), 1)
        self.assertEqual([str(t) for t in db_import[0].type.params], ["i32"] * 4)
        self.assertEqual([str(t) for t in db_import[0].type.results], ["i32"])
        connection = sqlite3.connect(":memory:")
        connection.execute("PRAGMA foreign_keys=ON")
        connection.executescript((ROOT / "coglets/factory/migrations/0001.sql").read_text())
        connection.execute("INSERT INTO objects VALUES (?,?,?,?,?,?,?)", (ID,"goal","open","{}",1,10,10))
        calls, logs = [], []
        linker = wasmtime.Linker(engine)
        linker.define_wasi()
        def db(caller, ptr, size, out, cap):
            memory = caller.get("memory")
            ptr, size, out, cap = [n & 0xffffffff for n in (ptr,size,out,cap)]
            self.assertGreaterEqual(cap,65540)
            self.assertLessEqual(ptr+size,memory.data_len(caller))
            self.assertLessEqual(out+cap,memory.data_len(caller))
            self.assertTrue(ptr+size <= out or out+cap <= ptr)
            raw = bytes(memory.read(caller,ptr,ptr+size))
            request = decode(raw)
            self.assertEqual(encode(request),raw)
            calls.append(request)
            if db_result:
                return db_result
            self.assertEqual(request["statement"],"objects_get")
            self.assertEqual(request["mode"],"read")
            cursor = connection.execute("SELECT id,type,status,data_json,version,created_at,updated_at FROM objects WHERE id=? ORDER BY id", request["params"])
            reply = encode(dict(status="page",columns=[c[0] for c in cursor.description], rows=[list(row) for row in cursor.fetchall()],affected=0,revision=7,next_cursor=None))
            memory.write(caller,len(reply).to_bytes(4,"big")+reply,out)
            return 0
        def log(caller,ptr,size):
            logs.append(decode(bytes(caller.get("memory").read(caller,ptr,ptr+size))))
            return 0
        def unused(*args):
            raise wasmtime.Trap("unexpected ordinary ABI operation")
        for imp in module.imports:
            if imp.module == "coglet_v0":
                self.assertIn(imp.name,["db","log","timer","emit","llm"])
                linker.define_func(imp.module, imp.name, imp.type, db if imp.name == "db" else log if imp.name == "log" else unused, access_caller=True)
        instance = linker.instantiate(store,module)
        exports = instance.exports(store)
        self.assertEqual(exports["memory"].type(store).limits.max,256)
        self.assertNotIn("_start",exports)
        if "_initialize" in exports: exports["_initialize"](store)
        event = dict(source="surface:"+"1"*64,kind="factory.inspect",seq=0,logical_time=10,payload={"id":ID},correlation_id="2"*32)
        raw = encode([event])
        ptr = exports["abi_alloc"](store,len(raw)) & 0xffffffff
        exports["memory"].write(store,raw,ptr)
        result = exports["handle"](store,ptr,len(raw),0,10,123)
        connection.close()
        return result,calls,logs
    def test_factory_reads_real_sqlite_through_wasm_db_import(self):
        result,calls,logs = self.run_guest()
        self.assertEqual(result,0)
        self.assertEqual(len(calls),1)
        self.assertEqual(logs[0]["fields"]["object_status"],"open")
        self.assertEqual(logs[0]["fields"]["object_id"],ID)
    def test_factory_propagates_db_rejection_to_guest_abort(self):
        result,calls,logs = self.run_guest(2)
        self.assertNotEqual(result,0)
        self.assertEqual(len(calls),1)
        self.assertEqual(logs,[])

if __name__ == "__main__": unittest.main()
