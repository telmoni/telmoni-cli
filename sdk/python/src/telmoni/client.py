from __future__ import annotations

from .models import Config


class Telmoni:
    def __init__(self, config: Config | None = None) -> None:
        self.config = config or Config()

    @classmethod
    def from_env(cls) -> Telmoni:
        return cls(Config.from_env())
