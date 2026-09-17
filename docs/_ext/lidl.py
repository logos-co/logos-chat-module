"""A Sphinx domain for LIDL contract declarations.

Gives each method, event and record of a contract a rendered signature that is
also a link target and a page-navigation entry, so the reference page needs no
heading above each declaration to be navigable.

Provides ``lidl:method``, ``lidl:event`` and ``lidl:record`` directives, and
matching roles for cross-referencing a declaration from prose.
"""

import re

from sphinx import addnodes
from sphinx.directives import ObjectDescription
from sphinx.domains import Domain, ObjType
from sphinx.roles import XRefRole
from sphinx.util.nodes import make_refnode

SIGNATURE = re.compile(
    r"""\A
    (?P<name>\w+)
    \s*(?:\((?P<params>.*)\))?
    \s*(?:->\s*(?P<returns>.+))?
    \Z""",
    re.VERBOSE | re.DOTALL,
)


class LidlDeclaration(ObjectDescription):
    def handle_signature(self, sig, signode):
        matched = SIGNATURE.match(sig.strip())
        if not matched:
            raise ValueError(f"unparsable LIDL signature: {sig}")
        name = matched.group("name")

        if self.objtype == "record":
            signode += addnodes.desc_annotation("type ", "type ")
        signode += addnodes.desc_name(name, name)

        params = matched.group("params")
        if params is not None:
            plist = addnodes.desc_parameterlist()
            for param in (p.strip() for p in params.split(",")):
                if param:
                    plist += addnodes.desc_parameter(param, param)
            signode += plist

        returns = matched.group("returns")
        if returns:
            signode += addnodes.desc_returns(returns, returns)

        signode["lidl_name"] = name
        return name

    def add_target_and_index(self, name, sig, signode):
        node_id = f"{self.objtype}-{name}" if self.objtype == "record" else name
        signode["ids"].append(node_id)
        domain = self.env.get_domain("lidl")
        domain.note_object(self.objtype, name, node_id, self.env.docname)

    def _object_hierarchy_parts(self, sig_node):
        return (sig_node.get("lidl_name", ""),)

    def _toc_entry_name(self, sig_node):
        name = sig_node.get("lidl_name")
        if not name:
            return ""
        return name if self.objtype == "record" else f"{name}()"


class LidlDomain(Domain):
    name = "lidl"
    label = "LIDL"
    object_types = {
        "method": ObjType("method", "method"),
        "event": ObjType("event", "event"),
        "record": ObjType("record", "record"),
    }
    directives = {
        "method": LidlDeclaration,
        "event": LidlDeclaration,
        "record": LidlDeclaration,
    }
    roles = {
        "method": XRefRole(),
        "event": XRefRole(),
        "record": XRefRole(),
    }
    initial_data = {"objects": {}}

    def note_object(self, objtype, name, node_id, docname):
        self.data["objects"][objtype, name] = (docname, node_id)

    def clear_doc(self, docname):
        for key, (owner, _) in list(self.data["objects"].items()):
            if owner == docname:
                del self.data["objects"][key]

    def merge_domaindata(self, docnames, otherdata):
        for key, value in otherdata["objects"].items():
            if value[0] in docnames:
                self.data["objects"][key] = value

    def resolve_xref(self, env, fromdocname, builder, typ, target, node, contnode):
        found = self.data["objects"].get((typ, target))
        if not found:
            return None
        docname, node_id = found
        return make_refnode(builder, fromdocname, docname, node_id, contnode, target)

    def get_objects(self):
        for (objtype, name), (docname, node_id) in self.data["objects"].items():
            yield name, name, objtype, docname, node_id, 1


def setup(app):
    app.add_domain(LidlDomain)
    return {"version": "1", "parallel_read_safe": True}
