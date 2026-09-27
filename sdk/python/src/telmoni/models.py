from __future__ import annotations

import os
from dataclasses import dataclass

DEFAULT_ENDPOINT = "https://telmoni.com"


@dataclass
class Config:
    endpoint: str = DEFAULT_ENDPOINT
    tenant_id: str | None = None
    auth_token: str | None = None

    def __post_init__(self) -> None:
        if self.endpoint:
            self.endpoint = self.endpoint.rstrip("/")

    @classmethod
    def from_env(cls) -> Config:
        endpoint = (
            os.getenv("TELMONI_ENDPOINT")
            or os.getenv("TELMONI_API_URL")
            or DEFAULT_ENDPOINT
        )
        auth_token = os.getenv("TELMONI_API_KEY") or os.getenv("TELMONI_AUTH_TOKEN")
        tenant_id = os.getenv("TELMONI_TENANT_ID") or os.getenv("TELMONI_TEAM_ID")
        return cls(
            endpoint=endpoint,
            auth_token=auth_token,
            tenant_id=tenant_id,
        )
