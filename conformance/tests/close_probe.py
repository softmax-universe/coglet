"""Child exit control for clean-close conformance regression tests."""
import sys

raise SystemExit(int(sys.argv[1]))
