"""Short, owned paths for Unix-domain socket lifecycle checks."""
from pathlib import Path
import tempfile

import pytest


@pytest.fixture
def socket_home():
    with tempfile.TemporaryDirectory(prefix='ad-') as directory:
        yield Path(directory)
