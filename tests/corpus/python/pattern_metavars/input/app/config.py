import os
import warnings

warnings.simplefilter("ignore")


def setting(name, fallback):
    value = os.environ.get(name)
    chosen = value if value else fallback
    # value if value else fallback  <- a comment, not code
    other = value if name else fallback
    return chosen, other
