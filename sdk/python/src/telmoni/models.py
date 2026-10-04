from __future__ import annotations

import os
from dataclasses import dataclass

DEFAULT_ENDPOINT = "https://telmoni.com"


@dataclass
class Config:
    endpoint: str = DEFAULT_ENDPOINT
    organization_id: str | None = None
    api_key: str | None = None

    def __post_init__(self) -> None:
        if self.endpoint:
            self.endpoint = self.endpoint.rstrip("/")

    @classmethod
    def from_env(cls) -> Config:
        endpoint = os.getenv("TELMONI_ENDPOINT") or DEFAULT_ENDPOINT
        api_key = os.getenv("TELMONI_API_KEY")
        organization_id = os.getenv("TELMONI_ORG")
        return cls(
            endpoint=endpoint,
            api_key=api_key,
            organization_id=organization_id,
        )
