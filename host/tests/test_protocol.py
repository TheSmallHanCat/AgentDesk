import importlib.util
import json
from dataclasses import asdict
from pathlib import Path

import jsonschema
import pytest

from agentdesk_host import __version__
from agentdesk_host.protocol import HelloParams, HelloRequest, ResumeCursor

ROOT = Path(__file__).resolve().parents[2]
SCHEMA = json.loads((ROOT / "protocol/hello.schema.json").read_text(encoding="utf-8"))


def test_package_version():
    assert __version__ == "0.1.0"


@pytest.mark.parametrize("example", json.loads((ROOT / "protocol/examples/hello.json").read_text(encoding="utf-8")))
def test_handshake_examples(example):
    jsonschema.validate(example, SCHEMA)


def test_invalid_handshake_is_rejected():
    example = json.loads((ROOT / "protocol/examples/hello.json").read_text(encoding="utf-8"))[0]
    example["params"]["protocolVersion"] = "2.0"
    with pytest.raises(jsonschema.ValidationError):
        jsonschema.validate(example, SCHEMA)


def test_generated_python_model_serializes_to_wire_shape():
    params = HelloParams(protocolVersion="1.0", client="agentdesk-desktop/0.1.0", deviceId="dev_example", deviceName="Development device", os="windows", resume=ResumeCursor(lastSeq=0))
    jsonschema.validate(asdict(HelloRequest(jsonrpc="2.0", id=1, method="hello", params=params)), SCHEMA)


def test_generator_rejects_unknown_schema_types():
    spec = importlib.util.spec_from_file_location("generate", ROOT / "protocol/generate.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    with pytest.raises(ValueError):
        module.field_type({"type": "mystery"}, "ts")
