#!/usr/bin/env python3
"""Serve a constant local page for the isolated native input-host check.

No project files, credentials or app handlers are exposed by this fixture.
"""
import argparse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

PAGE = b'<!doctype html><html lang="en"><meta charset="utf-8"><title>notdot input check</title><body>Native window check</body></html>'


class Fixture(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.send_header('Content-Type', 'text/html; charset=utf-8')
        self.send_header('Content-Length', str(len(PAGE)))
        self.end_headers()
        self.wfile.write(PAGE)

    def log_message(self, *_args):
        pass


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--port-file', type=Path, required=True)
    args = parser.parse_args()
    with ThreadingHTTPServer(('127.0.0.1', 0), Fixture) as server:
        temporary = args.port_file.with_suffix('.partial')
        temporary.write_text(str(server.server_port), encoding='ascii')
        temporary.replace(args.port_file)
        server.serve_forever()


if __name__ == '__main__':
    main()
