from __future__ import annotations

from enum import Enum


class Personality(str, Enum):
    NONE = "none"
    FRIENDLY = "friendly"
    PRAGMATIC = "pragmatic"


PersonalityLike = str | Personality


def personality_value(personality: PersonalityLike | None) -> str | None:
    if isinstance(personality, Personality):
        return personality.value
    return personality
