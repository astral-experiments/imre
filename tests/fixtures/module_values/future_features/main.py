import __future__ as features

# doctest finds active flags by comparing module feature objects by identity.
namespace = {"division": features.division, "annotations": features.annotations}
flags = 0
for name in features.all_feature_names:
    feature = getattr(features, name)
    if namespace.get(name) is feature:
        flags |= feature.compiler_flag
print(flags)
print(features.annotations.getOptionalRelease())
print(features.annotations.getMandatoryRelease())
print(features.generators.compiler_flag)
