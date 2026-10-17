import json
import socket
import threading
from pathlib import Path
from tempfile import TemporaryDirectory

import pytest


class FakeHerdr:
    def __init__(self, path):
        self.path = str(path)
        self.requests = []
        self.prompts = []
        self.agent = {
            "pane_id": "w1:p1",
            "terminal_id": "term-1",
            "workspace_id": "w1",
            "tab_id": "w1:t1",
            "agent": "claude",
            "cwd": "/project",
            "agent_session": {"source": "test", "kind": "id", "value": "s1", "agent": "claude"},
            "agent_status": "working",
            "name": "test-agent",
            "focused": False,
            "revision": 1,
        }
        self.prompt_error = None
        self.drop_prompt = False
        self.wrong_id = False
        self.fragment = False
        self.gate = threading.Event()
        self.gate.set()
        self.prompt_started = threading.Event()
        self.handler = None
        self.threads = []

    def connect(self, path, timeout):
        assert path == self.path
        client, server = socket.socketpair()
        client.settimeout(timeout)
        thread = threading.Thread(target=self.serve, args=(server,), daemon=True)
        self.threads.append(thread)
        thread.start()
        return client

    def respond(self, request):
        method = request["method"]
        if method == "agent.get":
            return {"result": {"type": "agent_info", "agent": self.agent}}
        if method == "agent.list":
            return {"result": {"type": "agent_list", "agents": [self.agent]}}
        if method == "session.snapshot":
            return {
                "result": {
                    "type": "session_snapshot",
                    "snapshot": {
                        "version": "0.9.3",
                        "protocol": 22,
                        "workspaces": [{"workspace_id": "w1", "label": "Projet test"}],
                        "tabs": [{"tab_id": "w1:t1", "label": "Développement"}],
                        "panes": [self.agent],
                        "layouts": [],
                        "agents": [self.agent],
                    },
                }
            }
        if method == "agent.prompt":
            self.prompt_started.set()
            self.gate.wait(3)
            if self.prompt_error:
                return {"error": self.prompt_error}
            self.prompts.append(request["params"]["text"])
            if self.drop_prompt:
                return None
            return {"result": {"type": "agent_prompted", "agent": self.agent}}
        return {"result": {"type": "ok"}}

    def serve(self, conn):
        with conn:
            try:
                data = b""
                while not data.endswith(b"\n"):
                    chunk = conn.recv(8192)
                    if not chunk:
                        break
                    data += chunk
                if not data:
                    return
                request = json.loads(data)
                self.requests.append(request)
                response = self.handler(request) if self.handler else self.respond(request)
                if response is None:
                    return
                response["id"] = "wrong" if self.wrong_id else request["id"]
                encoded = (json.dumps(response, ensure_ascii=False) + "\n").encode()
                if self.fragment:
                    for i in range(0, len(encoded), 7):
                        conn.sendall(encoded[i : i + 7])
                else:
                    conn.sendall(encoded)
            except (OSError, json.JSONDecodeError):
                pass

    def close(self):
        self.gate.set()
        for thread in self.threads:
            thread.join(2)


@pytest.fixture
def fake_herdr():
    with TemporaryDirectory(prefix="hd-", dir="/tmp") as directory:
        server = FakeHerdr(Path(directory) / "h.sock")
        yield server
        server.close()
