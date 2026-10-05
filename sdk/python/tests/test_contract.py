import os
import unittest
from unittest import mock

from telmoni import DEFAULT_ENDPOINT, Config, Telmoni


class TestTelmoniSdk(unittest.TestCase):
    def test_client_init(self) -> None:
        client = Telmoni(Config(endpoint="https://api.telmoni.local/"))
        self.assertEqual(client.config.endpoint, "https://api.telmoni.local")

    def test_client_default(self) -> None:
        client = Telmoni()
        self.assertEqual(client.config.endpoint, DEFAULT_ENDPOINT)
        self.assertEqual(client.config.endpoint, "https://telmoni.com")

    def test_repr_never_prints_the_key(self) -> None:
        config = Config(organization_id="org_1", api_key="telmoni_secret_key")
        self.assertNotIn("telmoni_secret_key", repr(config))
        self.assertIn("org_1", repr(config))
        self.assertEqual(Telmoni(config).config.api_key, "telmoni_secret_key")

    def test_client_from_env(self) -> None:
        # The shell running the suite may point the CLI at a local stack; the
        # SDK's default is what is under test.
        with mock.patch.dict(os.environ):
            os.environ.pop("TELMONI_ENDPOINT", None)
            client = Telmoni.from_env()
        self.assertEqual(client.config.endpoint, DEFAULT_ENDPOINT)


if __name__ == "__main__":
    unittest.main()
