import os,subprocess,time,urllib.request,urllib.error,json,signal
from pathlib import Path
import argparse
p=argparse.ArgumentParser();p.add_argument('--runtime',type=Path,required=True);args=p.parse_args()
project=Path(__file__).resolve().parent.parent
os.chdir(project)
root=args.runtime.resolve()
env={**os.environ,'PYTHONHOME':str(root/'python'),'PYTHONPATH':str(root/'.venv/lib/python3.13/site-packages'),'PYTHONDONTWRITEBYTECODE':'1','HF_HUB_OFFLINE':'1','TRANSFORMERS_OFFLINE':'1','QUIETTYPE_OLLAMA_HOST':'127.0.0.1:11439','QUIETTYPE_OLLAMA_BINARY':str(root/'ollama-runtime/ollama'),'OLLAMA_MODELS':str(root/'editor-models'),'OLLAMA_NO_CLOUD':'1'}
with Path('work/portable-runtime.log').open('w') as log:
 process=subprocess.Popen([root/'python/bin/python3.13','sidecar/local_speech/server.py','--models-dir',root,'--port','11438'],env=env,stdout=log,stderr=log)
 try:
  for attempt in range(60):
   try:
    health=json.load(urllib.request.urlopen('http://127.0.0.1:11438/health',timeout=2));break
   except OSError:time.sleep(.2)
  else:raise RuntimeError('Portable server failed to start')
  assert health['modelsDir']==str(root) and health['protocol']==3
  with urllib.request.urlopen(urllib.request.Request('http://127.0.0.1:11438/warm',method='POST'),timeout=180) as response:
   warm=json.load(response)
  assert warm.get('loaded'),warm
  assert any(x['name']=='qwen3.5:4b' for x in json.load(urllib.request.urlopen('http://127.0.0.1:11439/api/tags',timeout=3))['models'])
  request=urllib.request.Request('http://127.0.0.1:11438/transcribe',data=b'\x00'*32000,headers={'Content-Type':'application/octet-stream','X-Session-Id':'portable-silence'})
  try:
   silence=json.load(urllib.request.urlopen(request,timeout=30))
   assert silence.get('speech') is False,silence
  except urllib.error.HTTPError as error:
   assert error.code==400 and '未检测到声音' in error.read().decode()
  subprocess.run([root/'python/bin/python3.13','-c',
   'from mlx_audio.vad import load; import numpy as np,sys; model=load(sys.argv[1], strict=True); assert not model.get_speech_timestamps(np.zeros(16000,dtype=np.float32),sample_rate=16000)',str(root/'silero-vad')],env=env,check=True)
  print('Portable ASR + VAD + bundled private Ollama initialized offline')
 except urllib.error.HTTPError as error:
  print(error.read().decode(),flush=True)
  raise
 finally:
  process.terminate()
  try:process.wait(timeout=5)
  except subprocess.TimeoutExpired:process.kill();process.wait();raise
 for attempt in range(30):
  try:urllib.request.urlopen('http://127.0.0.1:11439/api/tags',timeout=1);time.sleep(.1)
  except OSError:break
 else:raise AssertionError('Bundled Ollama was left running after shutdown')
 print('SIGTERM reaped the bundled editor service')
