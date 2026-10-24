from setuptools.command.bdist_wheel import bdist_wheel


class ProjectWheel(bdist_wheel):
    def write_wheelfile(self, wheelfile_base, generator="retained-values"):
        super().write_wheelfile(wheelfile_base, generator)
