"""Adapter selection; a saved run keeps its original adapter."""
import herdr_codex
import herdr_claude


def select(name):
    adapters = {'herdr-codex': herdr_codex, 'herdr-claude': herdr_claude}
    try:
        return adapters[name]
    except KeyError:
        raise ValueError('Неизвестный адаптер: ' + str(name)) from None
