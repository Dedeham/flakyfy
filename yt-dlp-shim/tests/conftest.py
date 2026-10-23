"""The shim's lookup cache goes to a temporary file in tests, never to the deploy path."""
import os
import tempfile

os.environ.setdefault("META_DB_PATH", os.path.join(tempfile.mkdtemp(prefix="ytshim-test-"), "meta.db"))
