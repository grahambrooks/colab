import os

import requests


def fetch_data(url):
    return requests.get(os.path.join(url, "data"))
