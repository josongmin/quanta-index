"""Module docstring: def doc_decoy(): pass"""
# def comment_decoy(): pass
import functools


def target():
    def nested_target():
        return 1
    return nested_target


async def target_async():
    pass


@functools.lru_cache
def decorated():
    pass


class Widget:
    def method(self):
        pass

    @property
    def prop(self):
        return 1

    class Inner:
        pass


lam = lambda: 0
def target():  # a redefinition is a second declaration
    pass
def café():
    pass
def \
        continued():
    pass
if True:
    def conditional(): pass
TARGET = "def string_decoy(): pass"
def _(): pass
