var inputs = document.getElementsByClassName("select");
var sections = document.getElementsByClassName("section");

sections[0].classList.add("show");
for (let i = 1; i < inputs.length; i++) {
    sections[i].classList.add("hide");
}

for (let i = 0; i < inputs.length; i++) {
    inputs[i].onclick = () => {
        for (let j = 0; j < sections.length; j++) {
            sections[j].classList.remove("show");
            sections[j].classList.add("hide");
        }
        sections[i].classList.remove("hide");
        sections[i].classList.add("show");
    }
}