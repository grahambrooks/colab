from server import serve

CONFIG = {"host": "0.0.0.0", "port": 8080}
LOCAL = {"host": "localhost"}


def main():
    serve(CONFIG)
    serve(LOCAL, workers=4)
    serve(
        LOCAL,
    )
    debug = False
    return debug
