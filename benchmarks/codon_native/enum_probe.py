"""Observe all native conformance members, including identity and method dispatch."""

from conformance_report.model import Change, Classification, Source

for source in (Source.OLD, Source.NEW):
    print(source.name, source.value, str(source), source is Source.OLD)
for kind in (Classification.TRUE_POSITIVE, Classification.FALSE_POSITIVE,
             Classification.TRUE_NEGATIVE, Classification.FALSE_NEGATIVE):
    print(kind.name, kind.value, str(kind), kind == Classification.FALSE_NEGATIVE)
    print(kind.into_title(verb="added"), kind.into_title(verb="removed"), kind.into_title(verb="changed"))
for change in (Change.ADDED, Change.REMOVED, Change.CHANGED, Change.UNCHANGED):
    print(change.name, change.value, str(change), change.into_title())
