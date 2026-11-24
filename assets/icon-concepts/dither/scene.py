# ponytail: one-off scene: a commutator drawn like an old engraving (Workbench + Freestyle lines).
# blender -b -P scene.py  -> drawing.png (transparent)
import bpy, bmesh, math, os

HERE = os.path.dirname(os.path.abspath(__file__))
bpy.ops.wm.read_factory_settings(use_empty=True)
sc = bpy.context.scene
PARTS = []


def mat(name, color):
    m = bpy.data.materials.new(name)
    m.diffuse_color = (*color, 1)          # Workbench draws the viewport colour
    return m


# black and white: parts told apart by tone only
COPPER = mat("copper", (0.85, 0.85, 0.85))
MICA = mat("mica", (0.12, 0.12, 0.12))
STEEL = mat("steel", (0.5, 0.5, 0.5))


def put(ob, m):
    ob.data.materials.append(m)
    PARTS.append(ob)
    return ob


def sector(a0, a1, r0, r1, z0, z1, m, steps=8):
    bm = bmesh.new()
    pts = [(r1, a0 + (a1 - a0) * i / steps) for i in range(steps + 1)] + \
          [(r0, a1 - (a1 - a0) * i / steps) for i in range(steps + 1)]
    bm.faces.new([bm.verts.new((r * math.cos(a), r * math.sin(a), z0)) for r, a in pts])
    ext = bmesh.ops.extrude_face_region(bm, geom=bm.faces[:])
    bmesh.ops.translate(bm, vec=(0, 0, z1 - z0), verts=[v for v in ext["geom"] if isinstance(v, bmesh.types.BMVert)])
    bmesh.ops.recalc_face_normals(bm, faces=bm.faces)
    me = bpy.data.meshes.new("s")
    bm.to_mesh(me)
    ob = bpy.data.objects.new("s", me)
    sc.collection.objects.link(ob)
    return put(ob, m)


def cyl(r, z0, z1, m, verts=96):
    bpy.ops.mesh.primitive_cylinder_add(vertices=verts, radius=r, depth=z1 - z0, location=(0, 0, (z0 + z1) / 2))
    return put(bpy.context.object, m)


N = 20
step = 2 * math.pi / N
cyl(0.34, 0.08, 0.8, MICA)                                   # core under the bars
for i in range(N):
    a = i * step
    sector(a + 0.02, a + step - 0.02, 0.34, 0.5, 0.1, 0.8, COPPER)        # bars
    sector(a + 0.05, a + step - 0.05, 0.5, 0.68, 0.64, 0.8, COPPER)       # riser flange, slotted
sector(0, 2 * math.pi, 0.3, 0.52, 0.0, 0.1, STEEL, steps=96)            # front clamp ring
cyl(0.14, -0.55, 1.25, STEEL)                                 # shaft

# lay the drum on its side: axis along X, front end to the left
bpy.ops.object.empty_add(rotation=(0, math.radians(90), 0))
pivot = bpy.context.object
for ob in PARTS:
    ob.parent = pivot

bpy.ops.object.camera_add(location=(-3.0, -3.2, 1.9))
cam = bpy.context.object
cam.data.lens = 95
bpy.ops.object.empty_add(location=(0.3, 0, 0.0))
t = cam.constraints.new("TRACK_TO")
t.target, t.track_axis, t.up_axis = bpy.context.object, "TRACK_NEGATIVE_Z", "UP_Y"
sc.camera = cam

sc.render.engine = "BLENDER_WORKBENCH"
sh = sc.display.shading
# metal matcap: strong tonal range, which is what the dither turns into pattern
sh.light, sh.color_type = "MATCAP", "MATERIAL"
sh.studio_light = "fullmetal.exr"
sh.show_cavity, sh.cavity_type = True, "BOTH"
sh.show_object_outline = True
sh.object_outline_color = (0.0, 0.0, 0.0)

# engraving lines: silhouettes, creases and borders in near-black
sc.render.use_freestyle = True
sc.render.line_thickness_mode = "ABSOLUTE"
sc.render.line_thickness = 4.5
fs = sc.view_layers[0].freestyle_settings
lineset = fs.linesets[0] if len(fs.linesets) else fs.linesets.new("ink")
lineset.select_silhouette = lineset.select_crease = lineset.select_border = True
if lineset.linestyle is None:
    lineset.linestyle = bpy.data.linestyles.new("ink")
lineset.linestyle.color = (0.0, 0.0, 0.0)

sc.render.film_transparent = True
sc.render.resolution_x = sc.render.resolution_y = 1024
sc.view_settings.view_transform = "Standard"
sc.render.filepath = os.path.join(HERE, "drawing.png")
bpy.ops.render.render(write_still=True)
