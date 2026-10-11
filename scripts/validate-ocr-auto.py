"""Generate ignored OCR images, calibrate on one split, and report on a held-out split.

Requires Pillow only. Run generate, the ignored Rust corpus test, calibrate, rerun
that test, then report. No recognized text or image is printed or committed.
"""
import argparse
import json
from pathlib import Path
from statistics import mean
from PIL import Image, ImageDraw, ImageFont, ImageFilter

CALIBRATION = [
    ("zh-CN", "让阅读更流畅，让表达更清晰。"), ("zh-TW", "螢幕文字辨識與翻譯"),
    ("en-US", "Read the screen and translate this sentence."),
    ("en-GB", "Choose your favourite colour."), ("ja", "画面の文字を読み取ります。"),
    ("ko", "화면의 글자를 읽고 번역합니다"), ("de", "Größe und schöne Farben"),
    ("fr", "Écran français et caractères accentués"), ("es", "Información y traducción rápida"),
    ("it", "Traduzione dello schermo in italiano"), ("pt-PT", "Reconhecimento de ecrã e tradução"),
    ("pt-BR", "Informação em português brasileiro"),
    ("mixed", "你好世界 안녕하세요 Hello"), ("mixed", "日本語 English 화면"),
    ("short", "123.45%"), ("short", "OK"), ("ko", "한"),
]
VALIDATION = [
    ("zh-CN", "本地识别不需要网络连接"), ("zh-TW", "繁體中文測試與設定"),
    ("en-US", "Clipboard history keeps the complete text."),
    ("en-GB", "Organise the centre of the programme."), ("ja", "日本語の翻訳結果を表示します"),
    ("ko", "클립보드 기록에서 복사하세요"), ("de", "Überprüfung für zwölf Sprachen"),
    ("fr", "Déjà vu, où est la fenêtre ?"), ("es", "El niño escribe una canción"),
    ("it", "La qualità è più importante"), ("pt-PT", "A janela está disponível"),
    ("pt-BR", "Configurações da área de transferência"),
    ("mixed", "屏幕翻译 한국어 번역"), ("mixed", "日本語の文字 English text"),
    ("mixed", "你好안녕하세요世界"), ("mixed", "Hello 안녕하세요 world"),
    ("short", "007"), ("short", "!?"), ("ko", "글"),
]

def generate(root):
    root.mkdir(parents=True, exist_ok=True)
    entries=[]
    for split, samples in [("calibration",CALIBRATION),("validation",VALIDATION)]:
        for n,(language,text) in enumerate(samples):
            for size,blur in [(24,False),(34,False),(22,True)]:
                general=ImageFont.truetype("C:/Windows/Fonts/msyh.ttc",size)
                korean=ImageFont.truetype("C:/Windows/Fonts/malgun.ttf",size)
                fonts=[korean if 0xac00<=ord(ch)<=0xd7af else general for ch in text]
                width=sum(font.getlength(ch) for ch,font in zip(text,fonts))
                ascent=max(font.getmetrics()[0] for font in fonts)
                descent=max(font.getmetrics()[1] for font in fonts)
                image=Image.new("RGB",(int(width)+40,ascent+descent+24),"white")
                draw=ImageDraw.Draw(image);x=20
                for ch,font in zip(text,fonts):
                    draw.text((x,12+ascent),ch,font=font,fill="black",anchor="ls")
                    x+=font.getlength(ch)
                if blur: image=image.filter(ImageFilter.GaussianBlur(0.6))
                name=f"{split}-{n}-{size}.png";image.save(root/name)
                entries.append(dict(file=name,split=split,language=language,text=text))
    (root/"manifest.json").write_text(json.dumps(entries,ensure_ascii=False),encoding="utf-8")
    print("generated",len(entries),"images")

def distance(a,b):
    row=list(range(len(b)+1))
    for i,x in enumerate(a,1):
        new=[i]
        for j,y in enumerate(b,1):new.append(min(new[-1]+1,row[j]+1,row[j-1]+(x!=y)))
        row=new
    return row[-1]

def labels(tokens,expected):
    # Alignment labels, including inserted characters, calibrate correctness rather than confidence.
    actual="".join(t["text"] for t in tokens)
    d=[[0]*(len(expected)+1) for _ in range(len(actual)+1)]
    for i in range(len(actual)+1):d[i][0]=i
    for j in range(len(expected)+1):d[0][j]=j
    for i,x in enumerate(actual,1):
        for j,y in enumerate(expected,1):d[i][j]=min(d[i-1][j]+1,d[i][j-1]+1,d[i-1][j-1]+(x!=y))
    correct=[0]*len(actual);i,j=len(actual),len(expected)
    while i or j:
        if i and j and d[i][j]==d[i-1][j-1]+(actual[i-1]!=expected[j-1]):
            correct[i-1]=int(actual[i-1]==expected[j-1]);i-=1;j-=1
        elif i and d[i][j]==d[i-1][j]+1:i-=1
        else:j-=1
    offset=0
    for token in tokens:
        n=len(token["text"]);yield token["probability"],sum(correct[offset:offset+n])/max(n,1);offset+=n

def calibrate(root,output):
    records=json.loads((root/"results.json").read_text(encoding="utf-8"))
    result={"version":1,"minimum":0.75,"margin":0.08}
    for model in ["universal","korean"]:
        values=[]
        for record in records:
            if record["split"]=="calibration":values.extend(labels(record[model],record["expected"]))
        # Fixed probability bins with beta(1,1) smoothing, followed by isotonic pooling.
        groups=[]
        edges=[0,.25,.5,.65,.75,.85,.9,.95,.98,1.00001]
        for lo,hi in zip(edges,edges[1:]):
            ys=[y for x,y in values if lo<=x<hi]
            groups.append([lo,hi,sum(ys)+1,len(ys)+2])
            while len(groups)>1 and groups[-2][2]/groups[-2][3]>groups[-1][2]/groups[-1][3]:
                b=groups.pop();a=groups.pop();groups.append([a[0],b[1],a[2]+b[2],a[3]+b[3]])
        points=[[0,groups[0][2]/groups[0][3]]]
        points.extend([[(lo+hi)/2,yes/count] for lo,hi,yes,count in groups])
        points.append([1,groups[-1][2]/groups[-1][3]])
        result[model]=points
    def score(model,p):
        points=result[model]
        for a,b in zip(points,points[1:]):
            if p<=b[0]:return a[1]+max(0,min(1,(p-a[0])/(b[0]-a[0])))*(b[1]-a[1])
        return points[-1][1]
    def select(record,minimum,margin):
        u,k=record["universal"],record["korean"]
        selected=[(t["center"],t["text"]) for t in u]
        def hangul(t):return any(0xac00<=ord(ch)<=0xd7af or 0x1100<=ord(ch)<=0x11ff or 0x3130<=ord(ch)<=0x318f for ch in t["text"])
        accepted=[False]*len(k)
        index=0
        while index<len(k):
            if not hangul(k[index]):index+=1;continue
            start=index
            while index<len(k) and hangul(k[index]):index+=1
            run=k[start:index]
            repeats=sum(t["text"]==run[0]["text"] for t in run)
            if repeats>3 and repeats*2>len(run):continue
            gaps=sorted(b["center"]-a["center"] for a,b in zip(k,k[1:]) if b["center"]>a["center"])
            half=(gaps[len(gaps)//2] if gaps else 1)/2
            left=max(0,run[0]["center"]-half) if start==0 else (k[start-1]["center"]+run[0]["center"])/2
            right=min(1,run[-1]["center"]+half) if index==len(k) else (k[index]["center"]+run[-1]["center"])/2
            competing=[score("universal",x["probability"]) for x in u if left<=x["center"]<right and x["text"].strip()]
            other=sum(competing)/max(len(competing),len(run))
            confidence=mean(score("korean",t["probability"]) for t in run)
            if confidence<minimum or confidence<=other+margin:continue
            selected=[(pos,text) for pos,text in selected if pos<left or pos>=right or not text.strip()]
            selected.extend((t["center"],t["text"]) for t in run)
            accepted[start:index]=[True]*len(run)
        for i,t in enumerate(k):
            if t["text"].strip() or i==0 or i+1==len(k) or not accepted[i-1] or not accepted[i+1]:continue
            left,right=k[i-1]["center"],k[i+1]["center"]
            if not any(left<p<right and not text.strip() for p,text in selected):selected.append((t["center"],t["text"]))
        return "".join(text for _,text in sorted(selected))
    training=[r for r in records if r["split"]=="calibration"]
    choices=[]
    for minimum in [.7,.8,.9,.95]:
        for margin in [0,.01,.02,.04,.06,.08,.1]:
            errors=sum(distance(select(r,minimum,margin),r["expected"]) for r in training)
            choices.append((errors,-margin,-minimum,minimum,margin))
    best=min(choices)
    result["minimum"],result["margin"]=best[3:]
    result["calibration_edit_errors"]=best[0]
    result["resource_sha256"]=json.loads(Path("crates/lexift-platform/src/ocr/assets.json").read_text())["sha256"]
    result["calibration_images"]=sum(r["split"]=="calibration" for r in records)
    output.write_text(json.dumps(result,indent=2)+"\n",encoding="utf-8")
    print("calibration written; held-out images were excluded")

def report(root):
    records=json.loads((root/"results.json").read_text(encoding="utf-8"))
    result={}
    for split in ["calibration","validation"]:
        rows=[r for r in records if r["split"]==split]
        stats={}
        for field in ["selected","detected"]:
            stats[field+"_cer"]=sum(distance(r[field],r["expected"]) for r in rows)/sum(len(r["expected"]) for r in rows)
        for language in sorted({r["language"] for r in rows}):
            subset=[r for r in rows if r["language"]==language]
            stats[language+"_cer"]=sum(distance(r["detected"],r["expected"]) for r in subset)/sum(len(r["expected"]) for r in subset)
        timings=sorted(r["milliseconds"] for r in rows)
        stats.update(images=len(rows),mean_ms=mean(timings),p95_ms=timings[int(.95*(len(timings)-1))])
        result[split]=stats
    (root/"summary.json").write_text(json.dumps(result,indent=2),encoding="utf-8")
    print(json.dumps(result,indent=2))

if __name__=="__main__":
    p=argparse.ArgumentParser();p.add_argument("action",choices=["generate","calibrate","report"])
    p.add_argument("--root",type=Path,default=Path("target/ocr-auto-corpus"))
    p.add_argument("--output",type=Path,default=Path("crates/lexift-platform/src/ocr/calibration.json"))
    args=p.parse_args()
    if args.action=="generate":generate(args.root)
    elif args.action=="calibrate":calibrate(args.root,args.output)
    else:report(args.root)
