from .model import ItemDifference, MemoryItem


def item_total_bytes(item: MemoryItem) -> int:
    return item.metadata_bytes + item.fields_bytes


def item_change(item: ItemDifference) -> int:
    difference = item.new_bytes - item.old_bytes
    return -difference if difference < 0 else difference


def diff_items(
    old_items: list[MemoryItem], new_items: list[MemoryItem]
) -> list[ItemDifference]:
    old_by_name: dict[str, MemoryItem] = {item.name: item for item in old_items}
    new_by_name: dict[str, MemoryItem] = {item.name: item for item in new_items}
    names = list(old_by_name)
    for name in new_by_name:
        if name not in old_by_name:
            names.append(name)
    differences: list[ItemDifference] = []
    for name in names:
        old_item = old_by_name.get(name)
        new_item = new_by_name.get(name)
        old_bytes = item_total_bytes(old_item) if old_item is not None else 0
        new_bytes = item_total_bytes(new_item) if new_item is not None else 0
        if old_bytes != new_bytes:
            differences.append(ItemDifference(name.replace(" ", ""), old_bytes, new_bytes))
    # The original iterates a set union before stable sorting. Equal magnitudes
    # therefore depend on PYTHONHASHSEED; this port uses first appearance instead.
    return sorted(differences, key=item_change, reverse=True)
