"""Fetch exact HTTPS build inputs, with bounded transport retries."""

from http.client import IncompleteRead
from pathlib import Path
import socket
from ssl import SSLEOFError
from tempfile import TemporaryDirectory
import time
from urllib.error import HTTPError, URLError
from urllib.parse import urlsplit
from urllib.request import urlopen

from component_archive import sha256


class DownloadTransportError(OSError):
    pass


def download_io(phase, operation, *args, **kwargs):
    try:
        return operation(*args, **kwargs)
    except (URLError, TimeoutError, ConnectionError, IncompleteRead, SSLEOFError, socket.gaierror) as failure:
        # HTTP status errors, TLS verification failures, and permanent DNS errors are terminal.
        if isinstance(failure, HTTPError):
            raise
        cause = failure.reason if isinstance(failure, URLError) else failure
        if not (isinstance(cause, (TimeoutError, ConnectionError, IncompleteRead, SSLEOFError))
                or isinstance(cause, socket.gaierror) and cause.errno == socket.EAI_AGAIN):
            raise
        raise DownloadTransportError(f"{type(cause).__name__} while {phase}") from None


def fetch(directory, name, url, digest, size):
    destination = directory / name
    parsed = urlsplit(url)
    if parsed.scheme != "https" or parsed.username or parsed.password or not parsed.hostname:
        raise ValueError(f"{name}: expected an HTTPS download URL without credentials")
    if destination.exists() or destination.is_symlink():
        raise FileExistsError(destination)
    delays = (2, 5, None)
    for attempt, delay in enumerate(delays, start=1):
        context = f"{name} from {parsed.hostname} (attempt {attempt}/{len(delays)})"
        print(f"Fetching pinned {context}", flush=True)
        try:
            with TemporaryDirectory(prefix="download-", dir=directory) as scratch:
                temporary = Path(scratch) / name
                with download_io("opening HTTPS response", urlopen, url, timeout=30) as response, temporary.open("xb") as output:
                    if urlsplit(response.url).scheme != "https":
                        raise ValueError(f"{name}: download redirected away from HTTPS")
                    remaining = size
                    while chunk := download_io("reading HTTPS body", response.read, min(1024 * 1024, remaining + 1)):
                        remaining -= len(chunk)
                        if remaining < 0:
                            raise ValueError(f"{name}: download exceeded pinned size")
                        output.write(chunk)
                if temporary.stat().st_size != size or sha256(temporary) != digest:
                    raise ValueError(f"{name}: download differs from pinned size/SHA-256")
                # Windows rename is atomic within this directory and refuses to replace an input.
                temporary.rename(destination)
                return destination
        except DownloadTransportError as failure:
            if delay is None:
                raise DownloadTransportError(f"Fetching {context} failed: {failure}") from None
            print(f"Fetching {context} failed: {failure}. Retrying in {delay}s.", flush=True)
            time.sleep(delay)
