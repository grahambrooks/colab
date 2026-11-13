import os
import warnings



def setting(name, fallback):
    value = os.environ.get(name)
    chosen = value or fallback
    # value if value else fallback  <- a comment, not code
    other = value if name else fallback
    return chosen, other
