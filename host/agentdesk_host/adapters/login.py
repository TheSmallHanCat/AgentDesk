"""Ephemeral login prompts. This module never persists or logs native output."""

from __future__ import annotations

import re
from urllib.parse import urlsplit

# OSC includes hyperlink destinations; keep the visible URL, never control data.
_OSC = re.compile(r"\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)")
_CSI = re.compile(r"\x1b\[[0-?]*[ -/]*[@-~]")
_ESC = re.compile(r"\x1b(?:[()][0-2A-Z]|[@-_])")
_URL = re.compile(r"https?://[^\s<>\x00-\x1f]+(?=[\s<>\x00-\x1f])")


def clean_output(output: str) -> str:
    text = _ESC.sub("", _CSI.sub("", _OSC.sub("", output)))
    # Incomplete escape sequences remain hidden until the next PTY chunk.
    text = re.sub(r"\x1b[^\n]*$", "", text)
    text = text.replace("\r\n", "\n").replace("\r", "\n")
    return "".join(char for char in text if char in "\n\t" or ord(char) >= 32 and ord(char) != 127)


def safe_url(value: object) -> str | None:
    if not isinstance(value, str) or len(value) > 16384 or any(ord(char) < 33 for char in value):
        return None
    try:
        parsed = urlsplit(value)
        if parsed.scheme == "https" and parsed.hostname and not parsed.username and not parsed.password:
            return value
    except ValueError:
        pass
    return None


class LoginParser:
    """Incremental bounded buffer; subclasses own each CLI's output grammar."""

    def __init__(self):
        self.output = ""

    def feed(self, chunk: str) -> dict:
        self.output = (self.output + chunk)[-32768:]
        text = clean_output(self.output)
        result = self.parse(text)
        structured = result.get("state") or result.get("needsInput") or result.get("url") and result.get("deviceCode")
        if not structured and text.strip():
            result.update(rawOutput=text[-16384:], needsInput=True)
        return result

    def clear(self) -> None:
        self.output = ""

    def parse(self, text: str) -> dict:
        return {"needsInput": False}

    @staticmethod
    def url(text: str) -> str | None:
        for match in _URL.finditer(text):
            if value := safe_url(match.group().rstrip(".,);]")):
                return value
        return None

    @staticmethod
    def terminal_state(text: str) -> str | None:
        # "expires in 15 minutes" is an instruction, not an expiry notification.
        if re.search(r"\b(?:code|login|auth|authorization|authentication|session)\b[^\n]{0,80}\b(?:has expired|expired|timed out)\b|\b(?:expired_token|device_code_expired)\b", text, re.I):
            return "expired"
        if re.search(r"\b(?:error (?:logging in|during login)|login failed|authentication failed|authorization failed|failed to (?:log in|authenticate)|token exchange failed|access_denied)\b", text, re.I):
            return "failed"
        if re.search(r"\b(?:successfully logged in|login successful|authentication successful|logged in successfully)\b", text, re.I):
            return "succeeded"
        return None
