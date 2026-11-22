"""Exercise download retries and validation without making requests."""

from contextlib import redirect_stdout
from http.client import IncompleteRead
import hashlib
import io
from pathlib import Path
import socket
from ssl import SSLCertVerificationError
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import MagicMock, call, patch
from urllib.error import HTTPError, URLError

import pinned_setup_download as candidate


class PinnedSetupDownloadTests(unittest.TestCase):
    DATA = b"pinned setup input"
    URL = "https://example.com/qtbase.7z?private=do-not-log"
    NAME = "qtbase.7z"

    def setUp(self):
        scratch = self.enterContext(TemporaryDirectory())
        self.root = Path(scratch)
        self.destination = self.root / self.NAME
        self.digest = hashlib.sha256(self.DATA).hexdigest()
        self.open = self.enterContext(patch.object(candidate, "urlopen"))
        self.sleep = self.enterContext(patch.object(candidate.time, "sleep"))
        self.log = self.enterContext(redirect_stdout(io.StringIO()))

    def fetch(self, url=URL):
        return candidate.fetch(self.root, self.NAME, url, self.digest, len(self.DATA))

    def response(self, *chunks, url=URL):
        response = MagicMock()
        response.__enter__.return_value = response
        response.url = url
        response.read.side_effect = chunks
        return response

    def test_retries_a_timeout_then_a_partial_read_and_places_only_verified_bytes(self):
        partial = self.response(self.DATA[:5], IncompleteRead(b""))
        complete = self.response(self.DATA, b"")
        self.open.side_effect = [URLError(TimeoutError()), partial, complete]
        self.assertEqual(self.fetch(), self.destination)
        self.assertEqual(self.destination.read_bytes(), self.DATA)
        self.assertEqual(set(self.root.iterdir()), {self.destination})
        self.assertEqual(self.open.call_args_list, [call(self.URL, timeout=30)] * 3)
        self.assertEqual(self.sleep.call_args_list, [call(2), call(5)])
        self.assertIn("attempt 1/3", self.log.getvalue())
        self.assertIn("TimeoutError while opening HTTPS response. Retrying in 2s", self.log.getvalue())
        self.assertIn("IncompleteRead while reading HTTPS body. Retrying in 5s", self.log.getvalue())
        self.assertNotIn("do-not-log", self.log.getvalue())

    def test_transport_exhaustion_cleans_up_and_names_the_failed_input(self):
        self.open.side_effect = [
            self.response(self.DATA[:5], ConnectionResetError()),
            URLError(socket.gaierror(socket.EAI_AGAIN, "temporary DNS error")),
            TimeoutError(),
        ]
        with self.assertRaisesRegex(candidate.DownloadTransportError,
                                    r"qtbase\.7z from example\.com \(attempt 3/3\).*TimeoutError"):
            self.fetch()
        self.assertEqual(self.open.call_count, 3)
        self.assertEqual(self.sleep.call_count, 2)
        self.assertEqual(list(self.root.iterdir()), [])

    def test_integrity_errors_and_https_downgrades_fail_on_first_attempt(self):
        cases = [
            (self.response(self.DATA + b"x"), "exceeded pinned size"),
            (self.response(self.DATA[:-1], b""), "size/SHA-256"),
            (self.response(self.DATA[::-1], b""), "size/SHA-256"),
            (self.response(url="http://example.com/qtbase.7z"), "redirected away from HTTPS"),
        ]
        for response, message in cases:
            with self.subTest(message=message):
                self.open.reset_mock()
                self.open.return_value = response
                with self.assertRaisesRegex(ValueError, message):
                    self.fetch()
                self.open.assert_called_once()
                self.sleep.assert_not_called()
                self.assertEqual(list(self.root.iterdir()), [])

    def test_http_status_permanent_dns_and_certificate_errors_are_terminal(self):
        http_error = HTTPError(self.URL, 503, "http status", None, io.BytesIO())
        self.addCleanup(http_error.close)
        for error in (
            http_error,
            URLError(socket.gaierror(socket.EAI_NONAME, "permanent DNS error")),
            URLError(SSLCertVerificationError("certificate failure")),
        ):
            with self.subTest(error=type(error)):
                self.open.reset_mock()
                self.open.side_effect = error
                with self.assertRaises(URLError) as exc:
                    self.fetch()
                self.assertIs(exc.exception, error)
                self.open.assert_called_once()
                self.sleep.assert_not_called()
                self.assertEqual(list(self.root.iterdir()), [])

    def test_a_local_io_failure_does_not_repeat_the_request(self):
        self.open.return_value = self.response(self.DATA, b"")
        with patch.object(Path, "rename", side_effect=TimeoutError("local I/O")):
            with self.assertRaisesRegex(TimeoutError, "local I/O"):
                self.fetch()
        self.open.assert_called_once()
        self.sleep.assert_not_called()
        self.assertEqual(list(self.root.iterdir()), [])

    def test_rejects_an_existing_destination_and_non_https_before_opening_a_request(self):
        with self.assertRaisesRegex(ValueError, "HTTPS download URL without credentials"):
            self.fetch("http://example.com/qtbase.7z")
        with self.assertRaisesRegex(ValueError, "HTTPS download URL without credentials"):
            self.fetch("https://user:password@example.com/qtbase.7z")
        self.destination.write_bytes(b"existing file")
        with self.assertRaises(FileExistsError):
            self.fetch()
        self.assertEqual(self.destination.read_bytes(), b"existing file")
        self.open.assert_not_called()


if __name__ == "__main__":
    unittest.main()
