(function(){
  var t=localStorage.getItem('dp-theme');
  if(!t||t==='dark')t='midnight';
  else if(t==='light')t='arctic';
  else if(t==='nexus')t='clean';
  else if(t==='nexus-dark')t='clean-dark';
  document.documentElement.setAttribute('data-theme',t);
  document.documentElement.setAttribute('data-color-scheme',(t==='arctic'||t==='clean')?'light':'dark');
})();
