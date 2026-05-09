import new_pkg
import new_pkg.sub
import new_pkg.sub as sub
from new_pkg.sub import foo, bar
import unrelated


def main():
    return old_pkg.run()
