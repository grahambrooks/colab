import old_pkg
import old_pkg.sub
import old_pkg.sub as sub
from old_pkg.sub import foo, bar
import unrelated


def main():
    return old_pkg.run()
