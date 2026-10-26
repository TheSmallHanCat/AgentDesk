"""Single registry for daemon adapters and the capabilities they publish."""

from .claude import ClaudeAdapter
from .codex import CodexAdapter
from .fake import FakeAdapter


ADAPTER_FACTORIES = (ClaudeAdapter, CodexAdapter)


def registered_adapters(*, fake: bool = False):
    factories = (*ADAPTER_FACTORIES, FakeAdapter) if fake else ADAPTER_FACTORIES
    return {factory.kind: factory() for factory in factories}
