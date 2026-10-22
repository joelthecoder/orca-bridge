#!/usr/bin/env python3
"""Package the Script Filter and literal argv action as an importable Alfred workflow."""
import pathlib
import plistlib
import zipfile
root=pathlib.Path(__file__).resolve().parent.parent
out=root/'target/launchers/Orca Bridge.alfredworkflow'
out.parent.mkdir(parents=True,exist_ok=True)
search='8DDFA079-29D0-4CF4-9001-E66C2B90BB80'
open_id='F239BF4C-602C-4E4C-BE97-BC30EE98854B'
workflow={
    'bundleid':'dev.joel.orca-bridge.alfred','name':'Orca Bridge','createdby':'Joel',
    'description':'Search Orca sessions and open their exact conversation',
    'version':'0.1.0','disabled':False,'readme':'Requires make install. Type orca followed by a query. Native push pairing is not required for search.',
    'objects':[
        {'uid':search,'type':'alfred.workflow.input.scriptfilter','version':3,'config':{
            'keyword':'orca','title':'Search Orca sessions','subtext':'Titles, paths, branches, agents and models',
            'argumenttype':1,'withspace':True,'scriptargtype':1,'type':0,
            'script':(root/'integrations/alfred/search.sh').read_text(),'escaping':0,
            'runningsubtext':'Searching local history…','alfredfiltersresults':False,
            'queuedelaymode':0,'queuedelaycustom':0,'queuemode':1}},
        {'uid':open_id,'type':'alfred.workflow.action.script','version':3,'config':{
            'script':(root/'integrations/alfred/open.sh').read_text(),'type':0,'scriptargtype':1,'escaping':0}}
    ],
    'connections':{search:[{'destinationuid':open_id,'modifiers':0,'modifiersubtext':'','vitoclose':False}]},
    'uidata':{search:{'xpos':80,'ypos':80},open_id:{'xpos':360,'ypos':80}}
}
with zipfile.ZipFile(out,'w',zipfile.ZIP_DEFLATED) as z:
    z.writestr('info.plist',plistlib.dumps(workflow))
print(out)
