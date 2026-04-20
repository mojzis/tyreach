import json

from medium_app.shared import util


def run_a() -> str:
    return json.dumps(util("a"))
