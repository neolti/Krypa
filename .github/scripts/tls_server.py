"""Test-only HTTPS server with a self-signed certificate, serving tests/web-fixtures.

CI uses it to prove that Krypa shows the certificate error state and no page content.
Usage: tls_server.py PORT CERT KEY DIRECTORY
"""
import functools
import http.server
import ssl
import sys

port, cert, key, directory = int(sys.argv[1]), sys.argv[2], sys.argv[3], sys.argv[4]
handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=directory)
server = http.server.ThreadingHTTPServer(("127.0.0.1", port), handler)
context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
context.load_cert_chain(cert, key)
server.socket = context.wrap_socket(server.socket, server_side=True)
server.serve_forever()
