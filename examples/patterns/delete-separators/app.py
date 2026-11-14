from server import serve

CONFIG = {"host": "0.0.0.0", "debug": True, "port": 8080}
LOCAL = {"host": "localhost", "debug": True}


def main():
    serve(CONFIG, debug=True)
    serve(LOCAL, debug=True, workers=4)
    serve(
        LOCAL,
        debug=True,
    )
    debug = False
    return debug
